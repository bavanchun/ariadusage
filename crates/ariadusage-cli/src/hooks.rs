// Ported from CodexBar Sources/CodexBarCLI/CLIHooksCommand.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBarCLI/CLIHooksWatchCommand.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/CLICommonOptionsTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

//! CLI `hooks` command (`list`, `enable`, `disable`, `test`, `watch`).
//!
//! Security and testing invariants:
//! - Hook subprocesses execute only through [`HookCommandExecutor`] (backed by the engine's
//!   `Process` broker in production and [`ariadusage_engine::hooks::FakeHookCommandExecutor`] in tests).
//! - Provider observations in `hooks watch` go through [`HooksWatchObserver`] so unit and
//!   integration tests never touch real credentials, network endpoints, or hook binaries.
//! - Refresh failures in `hooks watch` are mapped onto coarse, redacted status labels
//!   (`cancelled`, `timeout`, `offline`, `network_error`, `auth_required`, `error`); raw provider
//!   error messages and HTTP bodies are never forwarded.

use std::collections::BTreeMap;
use std::io::Write;
use std::sync::Arc;
use std::time::Duration;

use ariadusage_core::config::Config;
use ariadusage_core::pipeline::{
    BoxFuture, FetchContext, FetchError, FetchInteraction, FetchPhase, FetchRuntime,
    TransportClass, fetch_outcome,
};
use ariadusage_core::providers;
use ariadusage_core::refresh::{
    HookEvent, HookEventType, HookProviderObservation, HookProviderStatus, HookTestResult,
    HookTransitionDetector, HookTransportErrorKind, HooksConfig, HooksWatchStopSignal,
    decode_hooks_watch_interval, decode_hooks_watch_provider_names, hook_account_discriminator,
    hook_account_display_name, hook_refresh_failure_status, hooks_watch_lanes,
    hooks_watch_providers, sample_hook_event, sleep_interruptibly, validate_hooks_watch_flags,
};
use ariadusage_engine::brokers::call::BrokerCall;
use ariadusage_engine::brokers::net::{NetBroker, NetConfig};
use ariadusage_engine::brokers::secret_store::SecretStore;
use ariadusage_engine::config_store::ConfigStore;
use ariadusage_engine::hooks::{
    DispatchOutcome, HookCommandExecutor, HookRunner, ProcessBrokerHookExecutor,
};
use ariadusage_engine::paths;
use ariadusage_engine::registry::ProviderRegistry;
use ariadusage_engine::status::ProviderStatusFetcher;
use ariadusage_engine::{AppPreferences, EngineRuntime, TokioSleeper, build_default_secret_store};
use ariadusage_protocol::usage::ProviderErrorKind;
use ariadusage_protocol::{ProviderId, SecretString, StatusIndicator};
use clap::{Args, Subcommand};
use jiff::Timestamp;
use serde::Serialize;
use tokio_util::sync::CancellationToken;

/// Extended `--help` description for `ariadusage hooks` (`CLIHelp.hooksHelp`).
pub const HOOKS_LONG_ABOUT: &str = "\
Run external commands when quota/provider events occur. Rules are stored in the
shared config file and are disabled by default. Events:
quota_low, quota_reached, quota_reset, usage_updated, provider_unavailable,
provider_recovered, refresh_failed.

Commands run directly (no shell), receive event metadata via ARIADUSAGE_* environment
variables and a JSON payload on stdin, and are timed out. Only configure commands you trust.

`watch` polls the selected providers and fires rules on real transitions, so hooks
work without the macOS app. Events are edge-triggered against the previous poll, so a
persisting condition does not re-fire. The first successful poll can emit usage_updated;
quota-transition baselines are established without firing. Keep one continuous process so transition
baselines and event rate limits survive between polls. Default interval 300s, minimum 60s.

Examples:
  ariadusage hooks list
  ariadusage hooks enable
  ariadusage hooks test quota_reached --provider codex
  ariadusage hooks test quota_low --provider claude
  ariadusage hooks watch --interval 600
  ariadusage hooks watch --provider codex";

/// Output format for `ariadusage hooks` subcommands (`OutputFormat` in `CLIOptions.swift`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum HooksOutputFormat {
    #[default]
    Text,
    Json,
}

impl HooksOutputFormat {
    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "text" => Some(Self::Text),
            "json" => Some(Self::Json),
            _ => None,
        }
    }
}

/// Shared output flags for `hooks` subcommands (`HooksOptions` in `CLIHooksCommand.swift`).
#[derive(Args, Debug, Clone, Default, PartialEq, Eq)]
pub struct HooksOutputArgs {
    /// Output format: text | json
    #[arg(long = "format")]
    pub format: Option<String>,

    /// Emit JSON
    #[arg(long = "json")]
    pub json: bool,

    /// Emit JSON only (suppress non-JSON output)
    #[arg(long = "json-only")]
    pub json_only: bool,

    /// Pretty-print JSON output
    #[arg(long = "pretty")]
    pub pretty: bool,
}

/// Resolved output preferences (`CLIOutputPreferences` in `CLIOutputPreferences.swift`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct HooksOutputPreferences {
    pub format: HooksOutputFormat,
    pub json_only: bool,
    pub pretty: bool,
}

impl HooksOutputPreferences {
    /// Explicit `--format` wins over `--json` / `--json-only` (`CLIOutputPreferences.resolveOutputFormat`).
    pub fn from_args(args: &HooksOutputArgs) -> Self {
        let json_only = args.json_only;
        let pretty = args.pretty;
        let json_shortcut = args.json || json_only;
        let format = if let Some(parsed) = args.format.as_deref().and_then(HooksOutputFormat::parse)
        {
            parsed
        } else if json_shortcut {
            HooksOutputFormat::Json
        } else {
            HooksOutputFormat::Text
        };
        Self {
            format,
            json_only,
            pretty,
        }
    }

    pub const fn uses_json_output(&self) -> bool {
        self.json_only || matches!(self.format, HooksOutputFormat::Json)
    }
}

/// Top-level `ariadusage hooks` command arguments.
#[derive(Args, Debug, Clone, Default)]
pub struct HooksCommandArgs {
    #[command(flatten)]
    pub output: HooksOutputArgs,

    #[command(subcommand)]
    pub subcommand: Option<HooksSubcommand>,
}

/// Subcommands under `ariadusage hooks` (`CLIHooksCommand.swift` + `CLIHooksWatchCommand.swift`).
#[derive(Subcommand, Debug, Clone)]
pub enum HooksSubcommand {
    /// List configured hooks
    List(HooksOutputArgs),

    /// Enable hooks
    Enable(HooksOutputArgs),

    /// Disable hooks
    Disable(HooksOutputArgs),

    /// Fire matching hooks for an event
    Test(HooksTestArgs),

    /// Poll providers and fire hooks on quota/status changes
    Watch(HooksWatchArgs),
}

/// Arguments for `ariadusage hooks test` (`HooksTestOptions` in `CLIHooksCommand.swift`).
#[derive(Args, Debug, Clone, Default, PartialEq, Eq)]
pub struct HooksTestArgs {
    /// Event name (e.g. quota_reached)
    pub event: Option<String>,

    /// Provider to query: codex|claude|antigravity
    #[arg(long = "provider")]
    pub provider: Option<String>,

    #[command(flatten)]
    pub output: HooksOutputArgs,
}

/// Arguments for `ariadusage hooks watch` (`HooksWatchOptions` in `CLIHooksWatchCommand.swift`).
#[derive(Args, Debug, Clone, Default, PartialEq, Eq)]
pub struct HooksWatchArgs {
    /// Poll period in seconds (default 300, minimum 60)
    #[arg(long = "interval", allow_hyphen_values = true)]
    pub interval: Option<String>,

    /// Provider to query: codex|claude|antigravity|both|all
    #[arg(long = "provider")]
    pub provider: Vec<String>,

    /// Print fetch diagnostics
    #[arg(long = "verbose")]
    pub verbose: bool,

    /// Hidden flag so `--once` is caught by `validate_hooks_watch_flags` with upstream's error message.
    #[arg(long = "once", hide = true)]
    pub once: bool,

    #[command(flatten)]
    pub output: HooksOutputArgs,
}

/// Error category reported in JSON error payloads (`CLIErrorKind` in `CLIErrorReporting.swift`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum HooksErrorKind {
    Args,
    Config,
    Provider,
    Runtime,
}

/// Structured error returned by `hooks` subcommands (`CLIErrorReporting.swift`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HooksCliError {
    pub code: u8,
    pub message: Option<String>,
    pub output: HooksOutputPreferences,
    pub kind: HooksErrorKind,
}

impl HooksCliError {
    pub fn failure(
        message: impl Into<String>,
        output: HooksOutputPreferences,
        kind: HooksErrorKind,
    ) -> Self {
        Self {
            code: 1,
            message: Some(message.into()),
            output,
            kind,
        }
    }

    pub const fn silent_failure(output: HooksOutputPreferences, kind: HooksErrorKind) -> Self {
        Self {
            code: 1,
            message: None,
            output,
            kind,
        }
    }

    pub const fn should_print_exit_error(&self) -> bool {
        self.code != 0 && self.message.is_some()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct ProviderErrorPayload<'a> {
    code: i32,
    message: &'a str,
    kind: HooksErrorKind,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct CliErrorProviderPayload<'a> {
    provider: &'static str,
    source: &'static str,
    error: ProviderErrorPayload<'a>,
}

/// Writes `error` to `stdout` (when JSON output is requested) or `stderr` (in text mode), matching
/// `CodexBarCLI.exit` in `CLIErrorReporting.swift`.
pub fn write_hooks_cli_error(
    error: &HooksCliError,
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
) {
    if !error.should_print_exit_error() {
        return;
    }
    let Some(message) = error.message.as_deref() else {
        return;
    };
    if error.output.uses_json_output() {
        let payload = [CliErrorProviderPayload {
            provider: "cli",
            source: "cli",
            error: ProviderErrorPayload {
                code: i32::from(error.code),
                message,
                kind: error.kind,
            },
        }];
        let _ = print_json(&payload, error.output.pretty, stdout);
    } else {
        let _ = writeln!(stderr, "{message}");
    }
}

fn encode_json<T: Serialize + ?Sized>(value: &T, pretty: bool) -> Option<String> {
    if pretty {
        serde_json::to_string_pretty(value).ok()
    } else {
        serde_json::to_string(value).ok()
    }
}

fn print_json<T: Serialize + ?Sized>(
    value: &T,
    pretty: bool,
    stdout: &mut dyn Write,
) -> std::io::Result<()> {
    if let Some(encoded) = encode_json(value, pretty) {
        writeln!(stdout, "{encoded}")?;
    }
    Ok(())
}

/// Maps an optional [`StatusIndicator`] onto [`HookProviderStatus`]
/// (`CLIHooksWatchCommand.hookProviderStatus`).
pub fn hook_provider_status(indicator: Option<StatusIndicator>) -> HookProviderStatus {
    indicator.map_or(HookProviderStatus::Unknown, HookProviderStatus::from)
}

/// Maps a [`FetchError`] onto a coarse [`HookTransportErrorKind`], ensuring raw provider error
/// descriptions or HTTP response bodies never leak into hook events (`CLIHooksWatchCommand.hookRefreshFailureStatus`).
pub fn classify_fetch_error_for_hook(error: &FetchError) -> HookTransportErrorKind {
    match error {
        FetchError::Cancelled => HookTransportErrorKind::Cancelled,
        FetchError::Classified(classified) => match classified.transport {
            Some(TransportClass::Cancelled) => HookTransportErrorKind::Cancelled,
            Some(TransportClass::Timeout) => HookTransportErrorKind::TimedOut,
            Some(
                TransportClass::NotConnected
                | TransportClass::ConnectionLost
                | TransportClass::CannotConnect
                | TransportClass::CannotFindHost
                | TransportClass::Dns,
            ) => HookTransportErrorKind::Offline,
            None if classified.kind == ProviderErrorKind::NetworkFailure => {
                HookTransportErrorKind::NetworkError
            }
            None => HookTransportErrorKind::Other,
        },
        FetchError::NoAvailableStrategy(_) => HookTransportErrorKind::Other,
    }
}

/// Returns the coarse, redacted failure status string for a [`FetchError`]
/// (`CLIHooksWatchCommand.hookRefreshFailureStatus`).
pub fn hook_refresh_failure_status_for_error(error: &FetchError) -> &'static str {
    hook_refresh_failure_status(classify_fetch_error_for_hook(error))
}

/// Reports a dispatched [`HookEvent`] to `stdout` (`CLIHooksWatchCommand.reportHookEvent`).
pub fn report_hook_event(
    event: &HookEvent,
    output: HooksOutputPreferences,
    stdout: &mut dyn Write,
) {
    if output.uses_json_output() {
        let _ = print_json(event, output.pretty, stdout);
        return;
    }
    let mut line = format!("{} {}", event.event.as_str(), event.provider);
    if let Some(window) = &event.window {
        line.push_str(&format!(" window={window}"));
    }
    if let Some(usage) = event.usage_percent {
        line.push_str(&format!(" usage={:.0}%", usage * 100.0));
    }
    if let Some(status) = &event.status {
        line.push_str(&format!(" status={status}"));
    }
    let _ = writeln!(stdout, "{line}");
}

/// Boundary for observing a single provider during a `hooks watch` poll.
///
/// Production uses [`EngineHooksWatchObserver`]; tests inject a deterministic fake observer.
pub trait HooksWatchObserver: Send + Sync {
    fn observe<'a>(
        &'a self,
        provider: &'a ProviderId,
        config: &'a Config,
        verbose: bool,
    ) -> BoxFuture<'a, HookProviderObservation>;
}

/// Production [`HooksWatchObserver`] backed by [`ProviderStatusFetcher`], [`ProviderRegistry`],
/// and [`fetch_outcome`] running in read-only background mode (`CLIHooksWatchCommand.hooksWatchObservation`).
pub struct EngineHooksWatchObserver {
    net: Arc<NetBroker>,
    status_fetcher: ProviderStatusFetcher,
    registry: ProviderRegistry,
    secret_store: Option<Arc<SecretStore>>,
    base_environment: BTreeMap<String, SecretString>,
}

impl std::fmt::Debug for EngineHooksWatchObserver {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EngineHooksWatchObserver")
            .field("has_secret_store", &self.secret_store.is_some())
            .finish()
    }
}

impl EngineHooksWatchObserver {
    pub fn from_environment(config: &Config) -> Result<Self, String> {
        let net = Arc::new(NetBroker::new(NetConfig::default()).map_err(|e| e.to_string())?);
        let disable_keyring = std::env::var_os("ARIADUSAGE_DISABLE_KEYRING").as_deref()
            == Some(std::ffi::OsStr::new("1"));
        let secret_store = build_default_secret_store(config, disable_keyring).ok();
        let base_environment = std::env::vars()
            .map(|(k, v)| (k, SecretString::new(v)))
            .collect();
        Ok(Self {
            net,
            status_fetcher: ProviderStatusFetcher::default(),
            registry: ProviderRegistry::default(),
            secret_store,
            base_environment,
        })
    }
}

impl HooksWatchObserver for EngineHooksWatchObserver {
    fn observe<'a>(
        &'a self,
        provider: &'a ProviderId,
        config: &'a Config,
        _verbose: bool,
    ) -> BoxFuture<'a, HookProviderObservation> {
        Box::pin(async move {
            let status_call = BrokerCall {
                interaction: FetchInteraction::Background,
                cancel: CancellationToken::new(),
                request_id: format!("hooks-watch-status-{}", provider.as_str()),
            };
            let status = if let Some(descriptor) = self.registry.descriptor(provider)
                && descriptor.supports_status_polling()
            {
                let indicator = self
                    .status_fetcher
                    .fetch_for_descriptor(&self.net, &status_call, descriptor)
                    .await
                    .ok()
                    .flatten()
                    .map(|summary| summary.status.indicator);
                hook_provider_status(indicator)
            } else {
                HookProviderStatus::Unknown
            };

            let mut prefs = AppPreferences::from_config(config);
            // `hooks watch` runs with `includeCredits = false` (`CLIHooksWatchCommand.swift:166`).
            prefs.show_optional_credits_and_extra_usage = false;
            let provider_cfg = config.provider_config(provider.as_str());
            let selected_account = ProviderRegistry::selected_account(provider_cfg);

            let inputs = EngineRuntime::resolve_plan_inputs_with_store(
                self.secret_store.as_ref(),
                provider,
                config,
                &prefs,
                self.base_environment.clone(),
                FetchInteraction::Background,
                CancellationToken::new(),
                format!("hooks-watch-secret-{}", provider.as_str()),
            )
            .await;

            if selected_account.is_some() && inputs.selected_account_token.is_none() {
                return HookProviderObservation::new(
                    provider.as_str(),
                    None,
                    Vec::new(),
                    None,
                    status,
                    Some(
                        hook_refresh_failure_status(HookTransportErrorKind::AuthRequired)
                            .to_string(),
                    ),
                );
            }

            let Some(spec) = self.registry.resolve_spec(provider, config, &inputs) else {
                return HookProviderObservation::new(
                    provider.as_str(),
                    None,
                    Vec::new(),
                    None,
                    status,
                    Some(hook_refresh_failure_status(HookTransportErrorKind::Other).to_string()),
                );
            };

            let cx = FetchContext {
                provider: provider.clone(),
                runtime: FetchRuntime::Cli,
                interaction: FetchInteraction::Background,
                phase: FetchPhase::Regular,
                request_id: format!("hooks-watch-{}", provider.as_str()),
                source_mode: spec.source_mode,
                env: spec.environment,
                include_credits: false,
                selected_token_account: spec.selected_account.as_ref().map(|a| a.id.clone()),
                cancel: CancellationToken::new(),
            };

            let sleeper = TokioSleeper;
            let outcome = fetch_outcome(provider, spec.plan.as_ref(), &cx, &sleeper).await;

            match outcome.result {
                Ok(result) => {
                    let usage = result.usage.scoped_to(provider.clone());
                    let hides_personal_info = prefs.hide_personal_info;
                    let account_discriminator = spec
                        .selected_account
                        .as_ref()
                        .map(|a| a.id.clone())
                        .or_else(|| hook_account_discriminator(provider, &usage));
                    let lanes = hooks_watch_lanes(
                        provider,
                        &usage,
                        config,
                        account_discriminator.as_deref(),
                        hides_personal_info,
                    );
                    let account_display =
                        hook_account_display_name(provider, &usage, hides_personal_info);
                    let secondary = usage.secondary.clone();
                    HookProviderObservation::new(
                        provider.as_str(),
                        account_display,
                        lanes,
                        secondary,
                        status,
                        None,
                    )
                    .with_successful_usage(Some(usage))
                    .with_account_discriminator(account_discriminator)
                }
                Err(err) => HookProviderObservation::new(
                    provider.as_str(),
                    None,
                    Vec::new(),
                    None,
                    status,
                    Some(hook_refresh_failure_status_for_error(&err).to_string()),
                ),
            }
        })
    }
}

fn load_config_from_store(
    config_store: &ConfigStore,
    output: HooksOutputPreferences,
) -> Result<Config, HooksCliError> {
    config_store
        .load_effective()
        .map_err(|err| HooksCliError::failure(err.to_string(), output, HooksErrorKind::Config))
}

fn default_config_store(output: HooksOutputPreferences) -> Result<ConfigStore, HooksCliError> {
    let path = paths::config_path()
        .map_err(|err| HooksCliError::failure(err.to_string(), output, HooksErrorKind::Config))?;
    Ok(ConfigStore::new(path))
}

/// Runs `ariadusage hooks list` against `config_store` (`CLIHooksCommand.runHooksList`).
pub fn run_hooks_list_with_store(
    args: &HooksOutputArgs,
    config_store: &ConfigStore,
    stdout: &mut dyn Write,
) -> Result<(), HooksCliError> {
    let output = HooksOutputPreferences::from_args(args);
    let config = load_config_from_store(config_store, output)?;
    let hooks = config.hooks_config().unwrap_or_default();
    crate::logging::logger(crate::logging::LogCategories::HOOKS).debug(
        "Listing configured hooks",
        Some(BTreeMap::from([
            ("enabled", hooks.enabled.to_string()),
            ("rules", hooks.events.len().to_string()),
        ])),
    );

    match output.format {
        HooksOutputFormat::Text => {
            let state_label = if hooks.enabled { "enabled" } else { "disabled" };
            let _ = writeln!(stdout, "Hooks: {state_label}");
            if hooks.events.is_empty() {
                let _ = writeln!(stdout, "No rules configured.");
            } else {
                for rule in &hooks.events {
                    let provider = rule.provider.as_deref().unwrap_or("any");
                    let state = if rule.enabled { "on" } else { "off" };
                    let mut parts = Vec::with_capacity(1 + rule.arguments.len());
                    parts.push(rule.executable.as_str());
                    parts.extend(rule.arguments.iter().map(String::as_str));
                    let command = parts.join(" ");
                    let _ = writeln!(
                        stdout,
                        "[{state}] {} provider={provider}: {command}",
                        rule.event.as_str()
                    );
                }
            }
        }
        HooksOutputFormat::Json => {
            let _ = print_json(&hooks, output.pretty, stdout);
        }
    }

    Ok(())
}

/// Runs `ariadusage hooks enable` or `ariadusage hooks disable` against `config_store`
/// (`CLIHooksCommand.runHooksSetEnabled`).
pub fn run_hooks_set_enabled_with_store(
    args: &HooksOutputArgs,
    enabled: bool,
    config_store: &ConfigStore,
    stdout: &mut dyn Write,
) -> Result<(), HooksCliError> {
    let output = HooksOutputPreferences::from_args(args);
    let mut config = load_config_from_store(config_store, output)?;
    let mut hooks = config.hooks_config().unwrap_or_default();
    hooks.enabled = enabled;
    config.set_hooks_config(Some(hooks.clone()));

    config_store
        .save(&config)
        .map_err(|err| HooksCliError::failure(err.to_string(), output, HooksErrorKind::Config))?;
    crate::logging::logger(crate::logging::LogCategories::HOOKS).debug(
        "Updated hooks enablement",
        Some(BTreeMap::from([("enabled", enabled.to_string())])),
    );

    match output.format {
        HooksOutputFormat::Text => {
            let state_label = if enabled { "enabled" } else { "disabled" };
            let _ = writeln!(stdout, "Hooks: {state_label}");
        }
        HooksOutputFormat::Json => {
            let _ = print_json(&hooks, output.pretty, stdout);
        }
    }

    Ok(())
}

/// Runs `ariadusage hooks test` with an injected [`HookCommandExecutor`], `config_store`,
/// `base_env`, and timestamp `now` (`CLIHooksCommand.runHooksTest`).
pub async fn run_hooks_test_with_executor(
    args: &HooksTestArgs,
    config_store: &ConfigStore,
    executor: &dyn HookCommandExecutor,
    base_env: &BTreeMap<String, String>,
    now: Timestamp,
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
) -> Result<(), HooksCliError> {
    let output = HooksOutputPreferences::from_args(&args.output);

    let Some(event_type) = args
        .event
        .as_deref()
        .filter(|s| !s.is_empty())
        .and_then(HookEventType::parse)
    else {
        let names = HookEventType::ALL
            .iter()
            .map(|t| t.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        return Err(HooksCliError::failure(
            format!("Unknown or missing event. Use one of: {names}."),
            output,
            HooksErrorKind::Args,
        ));
    };

    let Some(provider_desc) = args
        .provider
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .and_then(|raw| providers::find_by_id_str(&raw.to_ascii_lowercase()))
    else {
        return Err(HooksCliError::failure(
            "Unknown or missing provider. Use --provider <name>.",
            output,
            HooksErrorKind::Args,
        ));
    };
    let provider = provider_desc.id.as_str();

    let event = sample_hook_event(event_type, provider, now);
    let config = load_config_from_store(config_store, output)?;
    let hooks = config.hooks_config().unwrap_or_default();
    let rules = hooks.matching_rules(&event);

    if rules.is_empty() {
        let message = if hooks.enabled {
            format!(
                "No hook rule matches {} for {provider}.",
                event_type.as_str()
            )
        } else {
            "Hooks are disabled.".to_string()
        };
        return Err(HooksCliError::failure(
            message,
            output,
            HooksErrorKind::Config,
        ));
    }

    let mut results = Vec::with_capacity(rules.len());
    for rule in &rules {
        match HookRunner::run_with_executor(executor, rule, &event, base_env).await {
            Ok(exec_out) => {
                let trimmed = exec_out.stdout.trim();
                let stdout_opt = (!trimmed.is_empty()).then(|| trimmed.to_string());
                results.push(HookTestResult {
                    rule_id: rule.id.clone(),
                    executable: rule.executable.clone(),
                    event: event_type.as_str().to_string(),
                    provider: provider.to_string(),
                    success: true,
                    stdout: stdout_opt.clone(),
                    error: None,
                });
                if !output.uses_json_output() {
                    if let Some(text) = stdout_opt {
                        let _ = writeln!(stdout, "ran {}: OK — {text}", rule.executable);
                    } else {
                        let _ = writeln!(stdout, "ran {}: OK", rule.executable);
                    }
                }
            }
            Err(err) => {
                let summary = err.failure_summary();
                results.push(HookTestResult {
                    rule_id: rule.id.clone(),
                    executable: rule.executable.clone(),
                    event: event_type.as_str().to_string(),
                    provider: provider.to_string(),
                    success: false,
                    stdout: None,
                    error: Some(summary.clone()),
                });
                if !output.uses_json_output() {
                    let _ = writeln!(stderr, "ran {}: {summary}", rule.executable);
                }
            }
        }
    }

    if output.uses_json_output() {
        let _ = print_json(&results, output.pretty, stdout);
    }

    if results.iter().all(|r| r.success) {
        Ok(())
    } else {
        Err(HooksCliError::silent_failure(
            output,
            HooksErrorKind::Runtime,
        ))
    }
}

/// Runs `ariadusage hooks watch` with an injected [`HooksWatchObserver`], [`HookCommandExecutor`],
/// [`HooksWatchStopSignal`], and sleep callback (`CLIHooksWatchCommand.runHooksWatch`).
#[allow(clippy::too_many_arguments)]
pub async fn run_hooks_watch_with_observer<F, Fut>(
    args: &HooksWatchArgs,
    config_store: &ConfigStore,
    observer: &dyn HooksWatchObserver,
    executor: Arc<dyn HookCommandExecutor>,
    base_env: &BTreeMap<String, String>,
    stop: &HooksWatchStopSignal,
    mut sleep_fn: F,
    stdout: &mut dyn Write,
) -> Result<(), HooksCliError>
where
    F: FnMut(u64) -> Fut,
    Fut: std::future::Future<Output = ()>,
{
    let output = HooksOutputPreferences::from_args(&args.output);

    if let Err(err) = validate_hooks_watch_flags(args.once) {
        return Err(HooksCliError::failure(
            err.message,
            output,
            HooksErrorKind::Args,
        ));
    }

    let interval = match decode_hooks_watch_interval(args.interval.as_deref()) {
        Ok(value) => value,
        Err(err) => {
            return Err(HooksCliError::failure(
                err.message,
                output,
                HooksErrorKind::Args,
            ));
        }
    };

    let raw_providers: Vec<&str> = args.provider.iter().map(String::as_str).collect();
    let explicit_providers = match decode_hooks_watch_provider_names(&raw_providers) {
        Ok(value) => value,
        Err(err) => {
            return Err(HooksCliError::failure(
                err.message,
                output,
                HooksErrorKind::Args,
            ));
        }
    };

    let config = load_config_from_store(config_store, output)?;
    let hooks = config.hooks_config().unwrap_or_default();
    let providers = match hooks_watch_providers(explicit_providers, &config) {
        Ok(list) => list,
        Err(err) => {
            return Err(HooksCliError::failure(
                err.message,
                output,
                HooksErrorKind::Args,
            ));
        }
    };

    if !hooks.enabled {
        return Err(HooksCliError::failure(
            "Hooks are disabled. Run `ariadusage hooks enable` first.",
            output,
            HooksErrorKind::Config,
        ));
    }
    if hooks.events.is_empty() {
        return Err(HooksCliError::failure(
            "No hook rules configured. See `ariadusage hooks list`.",
            output,
            HooksErrorKind::Config,
        ));
    }

    let verbose = args.verbose;

    let mut detector = HookTransitionDetector::new();
    let runner = HookRunner::new(executor);

    if !output.uses_json_output() {
        let names = providers
            .iter()
            .map(ProviderId::as_str)
            .collect::<Vec<_>>()
            .join(", ");
        let _ = writeln!(
            stdout,
            "Watching {} provider(s) every {}s: {names}",
            providers.len(),
            interval as i64
        );
        let _ = writeln!(stdout, "Press Ctrl-C to stop.");
    }

    while !stop.is_requested() {
        for provider in &providers {
            if stop.is_requested() {
                break;
            }
            let observation = observer.observe(provider, &config, verbose).await;
            let now = Timestamp::now();
            let now = Timestamp::from_second(now.as_second()).unwrap_or(now);
            let dispatches = detector.evaluate(&observation, &hooks, now);
            for dispatch in dispatches {
                let dispatch_config = match dispatch.rules {
                    Some(rules) => HooksConfig::new(true, rules),
                    None => hooks.clone(),
                };
                let outcome = runner
                    .dispatch(
                        &dispatch.event,
                        &dispatch_config,
                        dispatch.account_discriminator.as_deref(),
                        base_env,
                        now,
                    )
                    .await;
                if outcome == DispatchOutcome::Attempted {
                    report_hook_event(&dispatch.event, output, stdout);
                }
            }
        }

        if stop.is_requested() {
            break;
        }
        sleep_interruptibly(interval, stop, &mut sleep_fn).await;
    }

    Ok(())
}

fn spawn_termination_signal_monitor(stop: Arc<HooksWatchStopSignal>) {
    #[cfg(unix)]
    {
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            handle.spawn(async move {
                use tokio::signal::unix::{SignalKind, signal};
                let mut sigint = signal(SignalKind::interrupt()).ok();
                let mut sigterm = signal(SignalKind::terminate()).ok();
                let mut sighup = signal(SignalKind::hangup()).ok();
                tokio::select! {
                    _ = async {
                        match &mut sigint {
                            Some(s) => { let _ = s.recv().await; }
                            None => std::future::pending::<()>().await,
                        }
                    } => {}
                    _ = async {
                        match &mut sigterm {
                            Some(s) => { let _ = s.recv().await; }
                            None => std::future::pending::<()>().await,
                        }
                    } => {}
                    _ = async {
                        match &mut sighup {
                            Some(s) => { let _ = s.recv().await; }
                            None => std::future::pending::<()>().await,
                        }
                    } => {}
                }
                stop.request();
            });
        }
    }
    #[cfg(not(unix))]
    {
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            handle.spawn(async move {
                let _ = tokio::signal::ctrl_c().await;
                stop.request();
            });
        }
    }
}

/// Entrypoint for `ariadusage hooks` invoked from `main.rs`.
pub fn run_hooks_command(args: HooksCommandArgs) -> Result<(), HooksCliError> {
    match args.subcommand {
        None => {
            let output = HooksOutputPreferences::from_args(&args.output);
            let store = default_config_store(output)?;
            let mut stdout = std::io::stdout();
            run_hooks_list_with_store(&args.output, &store, &mut stdout)
        }
        Some(HooksSubcommand::List(out_args)) => {
            let output = HooksOutputPreferences::from_args(&out_args);
            let store = default_config_store(output)?;
            let mut stdout = std::io::stdout();
            run_hooks_list_with_store(&out_args, &store, &mut stdout)
        }
        Some(HooksSubcommand::Enable(out_args)) => {
            let output = HooksOutputPreferences::from_args(&out_args);
            let store = default_config_store(output)?;
            let mut stdout = std::io::stdout();
            run_hooks_set_enabled_with_store(&out_args, true, &store, &mut stdout)
        }
        Some(HooksSubcommand::Disable(out_args)) => {
            let output = HooksOutputPreferences::from_args(&out_args);
            let store = default_config_store(output)?;
            let mut stdout = std::io::stdout();
            run_hooks_set_enabled_with_store(&out_args, false, &store, &mut stdout)
        }
        Some(HooksSubcommand::Test(test_args)) => {
            let output = HooksOutputPreferences::from_args(&test_args.output);
            let store = default_config_store(output)?;
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|err| {
                    HooksCliError::failure(err.to_string(), output, HooksErrorKind::Runtime)
                })?;
            let executor = ProcessBrokerHookExecutor::new();
            let base_env: BTreeMap<String, String> = std::env::vars().collect();
            let now = Timestamp::now();
            let now = Timestamp::from_second(now.as_second()).unwrap_or(now);
            let mut stdout = std::io::stdout();
            let mut stderr = std::io::stderr();
            runtime.block_on(run_hooks_test_with_executor(
                &test_args,
                &store,
                &executor,
                &base_env,
                now,
                &mut stdout,
                &mut stderr,
            ))
        }
        Some(HooksSubcommand::Watch(watch_args)) => {
            let output = HooksOutputPreferences::from_args(&watch_args.output);
            // Validate CLI flags before resolving or reading the config store so broken configs
            // never mask argument errors (`CLIHooksWatchCommand.swift:12-38`).
            if let Err(err) = validate_hooks_watch_flags(watch_args.once) {
                return Err(HooksCliError::failure(
                    err.message,
                    output,
                    HooksErrorKind::Args,
                ));
            }
            if let Err(err) = decode_hooks_watch_interval(watch_args.interval.as_deref()) {
                return Err(HooksCliError::failure(
                    err.message,
                    output,
                    HooksErrorKind::Args,
                ));
            }
            let raw_providers: Vec<&str> = watch_args.provider.iter().map(String::as_str).collect();
            if let Err(err) = decode_hooks_watch_provider_names(&raw_providers) {
                return Err(HooksCliError::failure(
                    err.message,
                    output,
                    HooksErrorKind::Args,
                ));
            }

            let store = default_config_store(output)?;
            let config = load_config_from_store(&store, output)?;
            let observer = EngineHooksWatchObserver::from_environment(&config)
                .map_err(|err| HooksCliError::failure(err, output, HooksErrorKind::Runtime))?;
            let executor: Arc<dyn HookCommandExecutor> = Arc::new(ProcessBrokerHookExecutor::new());
            let base_env: BTreeMap<String, String> = std::env::vars().collect();
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|err| {
                    HooksCliError::failure(err.to_string(), output, HooksErrorKind::Runtime)
                })?;
            let stop = Arc::new(HooksWatchStopSignal::new());
            let stop_for_signal = Arc::clone(&stop);
            let mut stdout = std::io::stdout();
            runtime.block_on(async move {
                spawn_termination_signal_monitor(stop_for_signal);
                run_hooks_watch_with_observer(
                    &watch_args,
                    &store,
                    &observer,
                    executor,
                    &base_env,
                    &stop,
                    |ns| tokio::time::sleep(Duration::from_nanos(ns)),
                    &mut stdout,
                )
                .await
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    use ariadusage_core::config::{
        ProviderConfig, ProviderEntry, QuotaWarningWindow, QuotaWarningWindowConfig, QuotaWarnings,
    };
    use ariadusage_core::model::{ProviderIdentitySnapshot, RateWindow, UsageSnapshot};
    use ariadusage_core::pipeline::ClassifiedError;
    use ariadusage_core::refresh::{HookQuotaLaneKey, HookRule};
    use ariadusage_engine::hooks::{FakeHookCommandExecutor, HookExecutionOutput, HookRunnerError};
    use ariadusage_protocol::Confidence;
    use clap::{CommandFactory, Parser};
    use tempfile::TempDir;

    #[derive(Parser, Debug)]
    struct TestHooksCli {
        #[command(subcommand)]
        command: TestTopCommand,
    }

    #[derive(Subcommand, Debug)]
    enum TestTopCommand {
        Hooks(HooksCommandArgs),
    }

    fn sample_usage_snapshot(
        primary: Option<RateWindow>,
        secondary: Option<RateWindow>,
        identity: Option<ProviderIdentitySnapshot>,
        now: Timestamp,
    ) -> UsageSnapshot {
        UsageSnapshot::new(
            primary,
            secondary,
            None,
            None,
            None,
            Vec::new(),
            None,
            None,
            now,
            identity,
            Confidence::Exact,
        )
        .expect("valid sample UsageSnapshot")
    }

    // Additional coverage for helper functions in `hooks.rs`
    #[test]
    fn watch_lanes_and_failure_status_helpers_cover_thresholds_and_redaction() {
        let now = Timestamp::from_second(1_700_000_000).expect("valid timestamp");
        let codex = ProviderId::new("codex").expect("valid provider id");
        let primary = RateWindow::new(30.0, None, None, None, None, false).expect("valid primary");
        let secondary =
            RateWindow::new(60.0, None, None, None, None, false).expect("valid secondary");
        let identity = ProviderIdentitySnapshot {
            provider_id: Some(codex.clone()),
            account_id: None,
            account_email: Some("user@example.com".to_string()),
            account_organization: None,
            login_method: None,
        };
        let usage = sample_usage_snapshot(Some(primary), Some(secondary), Some(identity), now);

        let mut codex_cfg = ProviderConfig::new(codex.clone());
        codex_cfg.quota_warnings = Some(QuotaWarnings {
            session: Some(QuotaWarningWindowConfig::new(Some(vec![25, 10]), None)),
            weekly: Some(QuotaWarningWindowConfig::new(Some(vec![40]), None)),
        });
        let config = Config::new(
            Config::CURRENT_VERSION,
            vec![ProviderEntry::Typed(codex_cfg)],
        );

        let lanes = hooks_watch_lanes(&codex, &usage, &config, None, false);
        assert_eq!(lanes.len(), 2);
        assert_eq!(
            lanes[0].key,
            HookQuotaLaneKey::new(
                "codex",
                QuotaWarningWindow::Session,
                Some("user@example.com".to_string()),
                None
            )
        );
        assert_eq!(lanes[0].fallback_thresholds, vec![0.75, 0.90]);
        assert_eq!(lanes[1].fallback_thresholds, vec![0.60]);

        assert_eq!(hook_provider_status(None), HookProviderStatus::Unknown);
        assert_eq!(
            hook_provider_status(Some(StatusIndicator::None)),
            HookProviderStatus::None
        );
        assert_eq!(
            hook_provider_status(Some(StatusIndicator::Major)),
            HookProviderStatus::Major
        );

        let non_url_err = FetchError::Classified(ClassifiedError::new(
            ProviderErrorKind::ApiFailure,
            "Bearer sk-secret",
        ));
        assert_eq!(hook_refresh_failure_status_for_error(&non_url_err), "error");
        assert_eq!(
            hook_refresh_failure_status_for_error(&FetchError::Cancelled),
            "cancelled"
        );
        let timeout_err = FetchError::Classified(
            ClassifiedError::new(ProviderErrorKind::NetworkFailure, "timed out")
                .with_transport(TransportClass::Timeout),
        );
        assert_eq!(
            hook_refresh_failure_status_for_error(&timeout_err),
            "timeout"
        );
        let offline_err = FetchError::Classified(
            ClassifiedError::new(ProviderErrorKind::NetworkFailure, "not connected")
                .with_transport(TransportClass::NotConnected),
        );
        assert_eq!(
            hook_refresh_failure_status_for_error(&offline_err),
            "offline"
        );
    }

    // upstream: Tests/CodexBarTests/CLICommonOptionsTests.swift:91 hooks output groups retain their own help and flags
    #[test]
    fn hooks_output_groups_retain_their_own_help_and_flags() {
        let root_cmd = TestHooksCli::command();
        let hooks_cmd = root_cmd
            .find_subcommand("hooks")
            .expect("hooks subcommand exists");
        for sub_name in ["test", "watch"] {
            let sub = hooks_cmd
                .find_subcommand(sub_name)
                .unwrap_or_else(|| panic!("{sub_name} subcommand exists"));
            for flag in ["format", "json", "json-only", "pretty"] {
                assert!(
                    sub.get_arguments().any(|a| a.get_long() == Some(flag)),
                    "expected flattened --{flag} on hooks {sub_name}"
                );
            }
            let json_arg = sub
                .get_arguments()
                .find(|a| a.get_long() == Some("json"))
                .expect("--json flag exists");
            assert_eq!(
                json_arg.get_help().map(ToString::to_string).as_deref(),
                Some("Emit JSON")
            );
        }

        for path in [
            vec!["hooks", "test", "quota_reached"],
            vec!["hooks", "watch"],
        ] {
            let mut argv = vec!["ariadusage"];
            argv.extend_from_slice(&path);
            argv.extend_from_slice(&["--format", "json", "--json", "--json-only", "--pretty"]);
            let parsed =
                TestHooksCli::try_parse_from(&argv).expect("hooks output flags must parse");
            match (path[1], parsed.command) {
                (
                    "test",
                    TestTopCommand::Hooks(HooksCommandArgs {
                        subcommand: Some(HooksSubcommand::Test(args)),
                        ..
                    }),
                ) => {
                    assert_eq!(args.event.as_deref(), Some("quota_reached"));
                    assert_eq!(args.output.format.as_deref(), Some("json"));
                    assert!(args.output.json);
                    assert!(args.output.json_only);
                    assert!(args.output.pretty);
                }
                (
                    "watch",
                    TestTopCommand::Hooks(HooksCommandArgs {
                        subcommand: Some(HooksSubcommand::Watch(args)),
                        ..
                    }),
                ) => {
                    assert_eq!(args.output.format.as_deref(), Some("json"));
                    assert!(args.output.json);
                    assert!(args.output.json_only);
                    assert!(args.output.pretty);
                }
                other => panic!("unexpected subcommand: {other:?}"),
            }
        }
    }

    #[test]
    fn explicit_format_wins_over_json_shortcut_in_hooks_output_preferences() {
        let explicit_text = HooksOutputPreferences::from_args(&HooksOutputArgs {
            format: Some("text".to_string()),
            json: true,
            json_only: false,
            pretty: false,
        });
        assert_eq!(explicit_text.format, HooksOutputFormat::Text);
        assert!(!explicit_text.uses_json_output());

        let explicit_text_with_json_only = HooksOutputPreferences::from_args(&HooksOutputArgs {
            format: Some("text".to_string()),
            json: false,
            json_only: true,
            pretty: false,
        });
        assert_eq!(explicit_text_with_json_only.format, HooksOutputFormat::Text);
        assert!(explicit_text_with_json_only.uses_json_output());

        let shortcut_json = HooksOutputPreferences::from_args(&HooksOutputArgs {
            format: None,
            json: true,
            json_only: false,
            pretty: true,
        });
        assert_eq!(shortcut_json.format, HooksOutputFormat::Json);
        assert!(shortcut_json.uses_json_output());
        assert!(shortcut_json.pretty);
    }

    #[test]
    fn hooks_list_enable_and_disable_round_trip_in_text_and_json() {
        let dir = TempDir::new().expect("tempdir");
        let store = ConfigStore::new(dir.path().join("config.json"));

        // Default list (disabled, no rules)
        let mut out = Vec::new();
        run_hooks_list_with_store(&HooksOutputArgs::default(), &store, &mut out)
            .expect("list should succeed");
        assert_eq!(
            String::from_utf8(out).expect("utf8"),
            "Hooks: disabled\nNo rules configured.\n"
        );

        // Enable hooks
        let mut out = Vec::new();
        run_hooks_set_enabled_with_store(&HooksOutputArgs::default(), true, &store, &mut out)
            .expect("enable should succeed");
        assert_eq!(String::from_utf8(out).expect("utf8"), "Hooks: enabled\n");

        // Save a rule and list in text and JSON
        let mut cfg = store.load_effective().expect("load config");
        let rule = HookRule::new(HookEventType::QuotaReached, "/usr/bin/echo")
            .with_id("rule-1")
            .with_provider("codex")
            .with_arguments(vec!["hello".to_string(), "world".to_string()]);
        cfg.set_hooks_config(Some(HooksConfig::new(true, vec![rule])));
        store.save(&cfg).expect("save config");

        let mut out = Vec::new();
        run_hooks_list_with_store(&HooksOutputArgs::default(), &store, &mut out)
            .expect("list with rule should succeed");
        assert_eq!(
            String::from_utf8(out).expect("utf8"),
            "Hooks: enabled\n[on] quota_reached provider=codex: /usr/bin/echo hello world\n"
        );

        let mut json_out = Vec::new();
        run_hooks_list_with_store(
            &HooksOutputArgs {
                json: true,
                ..HooksOutputArgs::default()
            },
            &store,
            &mut json_out,
        )
        .expect("json list should succeed");
        let decoded: HooksConfig =
            serde_json::from_slice(&json_out).expect("valid HooksConfig JSON");
        assert!(decoded.enabled);
        assert_eq!(decoded.events.len(), 1);
        assert_eq!(decoded.events[0].id, "rule-1");

        // Disable hooks in JSON mode
        let mut disable_out = Vec::new();
        run_hooks_set_enabled_with_store(
            &HooksOutputArgs {
                json: true,
                ..HooksOutputArgs::default()
            },
            false,
            &store,
            &mut disable_out,
        )
        .expect("disable should succeed");
        let disabled_cfg: HooksConfig =
            serde_json::from_slice(&disable_out).expect("valid HooksConfig JSON");
        assert!(!disabled_cfg.enabled);
    }

    #[tokio::test]
    async fn hooks_test_executes_matching_rules_via_fake_executor_without_spawning_processes() {
        let dir = TempDir::new().expect("tempdir");
        let store = ConfigStore::new(dir.path().join("config.json"));
        let now = Timestamp::from_second(1_700_000_000).expect("valid ts");

        let rule = HookRule::new(HookEventType::QuotaReached, "/bin/echo")
            .with_id("echo-rule")
            .with_provider("codex")
            .with_arguments(vec!["quota-hit".to_string()]);
        let mut cfg = Config::new(Config::CURRENT_VERSION, Vec::new());
        cfg.set_hooks_config(Some(HooksConfig::new(true, vec![rule])));
        store.save(&cfg).expect("save config");

        let executor = FakeHookCommandExecutor::new();
        let base_env = BTreeMap::from([
            ("PATH".to_string(), "/usr/bin:/bin".to_string()),
            ("SECRET_API_KEY".to_string(), "never-forward".to_string()),
        ]);

        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        run_hooks_test_with_executor(
            &HooksTestArgs {
                event: Some("quota_reached".to_string()),
                provider: Some("codex".to_string()),
                output: HooksOutputArgs::default(),
            },
            &store,
            &executor,
            &base_env,
            now,
            &mut stdout,
            &mut stderr,
        )
        .await
        .expect("hooks test should succeed");

        assert_eq!(
            String::from_utf8(stdout).expect("utf8"),
            "ran /bin/echo: OK — quota-hit\n"
        );
        assert!(stderr.is_empty());

        let requests = executor.requests();
        assert_eq!(requests.len(), 1);
        assert_eq!(
            requests[0]
                .environment
                .get("ARIADUSAGE_EVENT")
                .map(String::as_str),
            Some("quota_reached")
        );
        assert_eq!(
            requests[0]
                .environment
                .get("ARIADUSAGE_PROVIDER")
                .map(String::as_str),
            Some("codex")
        );
        assert!(!requests[0].environment.contains_key("SECRET_API_KEY"));
    }

    #[tokio::test]
    async fn hooks_test_reports_errors_and_failed_rules_in_text_and_json() {
        let dir = TempDir::new().expect("tempdir");
        let store = ConfigStore::new(dir.path().join("config.json"));
        let now = Timestamp::from_second(1_700_000_000).expect("valid ts");
        let executor = FakeHookCommandExecutor::with_handler(|_| {
            Ok(HookExecutionOutput {
                exit_code: 1,
                stdout: String::new(),
                stderr: String::new(),
            })
        });
        let base_env = BTreeMap::new();

        // Unknown event
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let err = run_hooks_test_with_executor(
            &HooksTestArgs {
                event: Some("not_an_event".to_string()),
                provider: Some("codex".to_string()),
                output: HooksOutputArgs::default(),
            },
            &store,
            &executor,
            &base_env,
            now,
            &mut stdout,
            &mut stderr,
        )
        .await
        .expect_err("unknown event must fail");
        assert_eq!(err.kind, HooksErrorKind::Args);
        assert!(
            err.message
                .as_deref()
                .unwrap_or_default()
                .starts_with("Unknown or missing event. Use one of: ")
        );

        // Unknown provider
        let err = run_hooks_test_with_executor(
            &HooksTestArgs {
                event: Some("quota_reached".to_string()),
                provider: Some("unknown".to_string()),
                output: HooksOutputArgs::default(),
            },
            &store,
            &executor,
            &base_env,
            now,
            &mut stdout,
            &mut stderr,
        )
        .await
        .expect_err("unknown provider must fail");
        assert_eq!(err.kind, HooksErrorKind::Args);
        assert_eq!(
            err.message.as_deref(),
            Some("Unknown or missing provider. Use --provider <name>.")
        );

        // Disabled hooks
        let err = run_hooks_test_with_executor(
            &HooksTestArgs {
                event: Some("quota_reached".to_string()),
                provider: Some("codex".to_string()),
                output: HooksOutputArgs::default(),
            },
            &store,
            &executor,
            &base_env,
            now,
            &mut stdout,
            &mut stderr,
        )
        .await
        .expect_err("disabled hooks must fail");
        assert_eq!(err.kind, HooksErrorKind::Config);
        assert_eq!(err.message.as_deref(), Some("Hooks are disabled."));

        // Failing rule in JSON mode
        let failing_exec =
            FakeHookCommandExecutor::with_handler(|_| Err(HookRunnerError::NonZeroExit(7)));
        let rule = HookRule::new(HookEventType::QuotaReached, "/usr/bin/false").with_id("fail-1");
        let mut cfg = Config::new(Config::CURRENT_VERSION, Vec::new());
        cfg.set_hooks_config(Some(HooksConfig::new(true, vec![rule])));
        store.save(&cfg).expect("save config");

        let mut json_stdout = Vec::new();
        let mut json_stderr = Vec::new();
        let err = run_hooks_test_with_executor(
            &HooksTestArgs {
                event: Some("quota_reached".to_string()),
                provider: Some("codex".to_string()),
                output: HooksOutputArgs {
                    json: true,
                    ..HooksOutputArgs::default()
                },
            },
            &store,
            &failing_exec,
            &base_env,
            now,
            &mut json_stdout,
            &mut json_stderr,
        )
        .await
        .expect_err("failing hook rule must return non-zero");
        assert!(!err.should_print_exit_error());
        let results: Vec<HookTestResult> =
            serde_json::from_slice(&json_stdout).expect("valid HookTestResult JSON");
        assert_eq!(results.len(), 1);
        assert!(!results[0].success);
        assert_eq!(results[0].error.as_deref(), Some("exit 7"));
    }

    struct SequenceWatchObserver {
        observations: Mutex<Vec<HookProviderObservation>>,
    }

    impl SequenceWatchObserver {
        fn new(observations: Vec<HookProviderObservation>) -> Self {
            Self {
                observations: Mutex::new(observations),
            }
        }
    }

    impl HooksWatchObserver for SequenceWatchObserver {
        fn observe<'a>(
            &'a self,
            provider: &'a ProviderId,
            _config: &'a Config,
            _verbose: bool,
        ) -> BoxFuture<'a, HookProviderObservation> {
            Box::pin(async move {
                let mut guard = self.observations.lock().expect("sequence observer lock");
                if guard.is_empty() {
                    HookProviderObservation::new(
                        provider.as_str(),
                        None,
                        Vec::new(),
                        None,
                        HookProviderStatus::None,
                        None,
                    )
                } else {
                    guard.remove(0)
                }
            })
        }
    }

    #[tokio::test]
    async fn hooks_watch_dispatches_transitions_and_stops_on_signal() {
        let dir = TempDir::new().expect("tempdir");
        let store = ConfigStore::new(dir.path().join("config.json"));

        // Empty enabled provider set reports kind = Args ("No providers are enabled.")
        let mut disabled_entries = Vec::new();
        for desc in providers::first_party_order() {
            let mut p = ProviderConfig::new(desc.id.clone());
            p.enabled = Some(false);
            disabled_entries.push(ProviderEntry::Typed(p));
        }
        store
            .save(&Config::new(Config::CURRENT_VERSION, disabled_entries))
            .expect("save disabled providers config");
        let empty_err = run_hooks_watch_with_observer(
            &HooksWatchArgs::default(),
            &store,
            &SequenceWatchObserver::new(Vec::new()),
            Arc::new(FakeHookCommandExecutor::new()),
            &BTreeMap::new(),
            &HooksWatchStopSignal::new(),
            |_ns| async {},
            &mut Vec::new(),
        )
        .await
        .expect_err("no enabled providers must fail");
        assert_eq!(empty_err.kind, HooksErrorKind::Args);
        assert_eq!(
            empty_err.message.as_deref(),
            Some("No providers are enabled.")
        );

        let codex = ProviderId::new("codex").expect("valid id");
        let mut codex_cfg = ProviderConfig::new(codex.clone());
        codex_cfg.enabled = Some(true);
        let rule = HookRule::new(HookEventType::ProviderUnavailable, "/usr/bin/true")
            .with_id("unavail-rule")
            .with_provider("codex");
        let mut cfg = Config::new(
            Config::CURRENT_VERSION,
            vec![ProviderEntry::Typed(codex_cfg)],
        );
        cfg.set_hooks_config(Some(HooksConfig::new(true, vec![rule])));
        store.save(&cfg).expect("save config");

        let obs1 = HookProviderObservation::new(
            "codex",
            None,
            Vec::new(),
            None,
            HookProviderStatus::None,
            None,
        );
        let obs2 = HookProviderObservation::new(
            "codex",
            None,
            Vec::new(),
            None,
            HookProviderStatus::Major,
            None,
        );
        let observer = SequenceWatchObserver::new(vec![obs1, obs2]);
        let executor = Arc::new(FakeHookCommandExecutor::new());
        let executor_trait: Arc<dyn HookCommandExecutor> =
            Arc::clone(&executor) as Arc<dyn HookCommandExecutor>;
        let stop = Arc::new(HooksWatchStopSignal::new());
        let stop_in_sleep = Arc::clone(&stop);
        let mut poll_sleeps = 0usize;
        let mut stdout = Vec::new();

        run_hooks_watch_with_observer(
            &HooksWatchArgs {
                interval: Some("60".to_string()),
                provider: vec!["codex".to_string()],
                ..HooksWatchArgs::default()
            },
            &store,
            &observer,
            executor_trait,
            &BTreeMap::new(),
            &stop,
            move |_ns| {
                poll_sleeps += 1;
                if poll_sleeps > 300 {
                    // After the first full 60s sleep (300 * 200ms ticks), let the second poll run,
                    // then stop on the first tick of the second sleep.
                    stop_in_sleep.request();
                }
                async {}
            },
            &mut stdout,
        )
        .await
        .expect("watch should exit cleanly on stop signal");

        let text = String::from_utf8(stdout).expect("utf8");
        assert!(text.contains("Watching 1 provider(s) every 60s: codex\n"));
        assert!(text.contains("Press Ctrl-C to stop.\n"));
        assert!(text.contains("provider_unavailable codex status=major\n"));
        assert_eq!(executor.requests().len(), 1);
    }
}
