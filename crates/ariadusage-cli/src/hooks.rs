// Ported from CodexBar Sources/CodexBarCLI/CLIHooksCommand.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBarCLI/CLIHooksWatchCommand.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/CLIHooksTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/CLICommonOptionsTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar TestsLinux/CLIHooksWatchSleepLinuxTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar TestsLinux/CLIHooksWatchArgumentPrecedenceLinuxTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

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
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use ariadusage_core::config::Config;
use ariadusage_core::pipeline::{
    BoxFuture, FetchContext, FetchError, FetchInteraction, FetchPhase, FetchRuntime,
    TransportClass, fetch_outcome,
};
use ariadusage_core::providers;
use ariadusage_core::refresh::{
    HookEvent, HookEventType, HookProviderObservation, HookProviderStatus, HookRateLimiter,
    HookTestResult, HookTransitionDetector, HookTransportErrorKind, HooksConfig,
    HooksWatchStopSignal, decode_hooks_watch_interval, decode_hooks_watch_provider_names,
    decode_hooks_watch_web_timeout, hook_account_discriminator, hook_account_display_name,
    hook_refresh_failure_status, hooks_watch_lanes, hooks_watch_providers, sample_hook_event,
    sleep_interruptibly, validate_hooks_watch_flags,
};
use ariadusage_engine::brokers::call::BrokerCall;
use ariadusage_engine::brokers::net::{NetBroker, NetConfig};
use ariadusage_engine::brokers::secret_store::file::FileBackend;
use ariadusage_engine::brokers::secret_store::service::ServiceBackend;
use ariadusage_engine::brokers::secret_store::{SecretId, SecretLookup, SecretStore};
use ariadusage_engine::config_store::ConfigStore;
use ariadusage_engine::hooks::{
    DispatchOutcome, HookCommandExecutor, HookRunner, ProcessBrokerHookExecutor,
};
use ariadusage_engine::paths;
use ariadusage_engine::registry::{ProviderPlanInputs, ProviderRegistry};
use ariadusage_engine::status::ProviderStatusFetcher;
use ariadusage_engine::{AppPreferences, TokioSleeper};
use ariadusage_protocol::ids::SettingId;
use ariadusage_protocol::usage::ProviderErrorKind;
use ariadusage_protocol::{ProviderId, SecretString, StatusIndicator};
use clap::{Args, Subcommand};
use jiff::Timestamp;
use serde::Serialize;
use tokio_util::sync::CancellationToken;

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
    pub fn from_args(args: &HooksOutputArgs) -> Self {
        let json_only = args.json_only;
        let pretty = args.pretty;
        let format = if json_only || args.json {
            HooksOutputFormat::Json
        } else {
            args.format
                .as_deref()
                .and_then(HooksOutputFormat::parse)
                .unwrap_or(HooksOutputFormat::Text)
        };
        Self {
            format,
            json_only,
            pretty,
        }
    }

    pub const fn uses_json_output(&self) -> bool {
        matches!(self.format, HooksOutputFormat::Json) || self.json_only
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

    /// Web fetch timeout (seconds)
    #[arg(long = "web-timeout", allow_hyphen_values = true)]
    pub web_timeout: Option<String>,

    /// Print fetch diagnostics
    #[arg(short = 'v', long = "verbose")]
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
        web_timeout: f64,
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
        let consented = config.secret_file_fallback == Some(true);
        let disable_keyring = std::env::var_os("ARIADUSAGE_DISABLE_KEYRING").as_deref()
            == Some(std::ffi::OsStr::new("1"));
        let secret_store = paths::data_dir().ok().map(|data_dir| {
            let file_backend =
                FileBackend::new(data_dir.join("ariadusage").join("secrets.json"), consented);
            Arc::new(
                SecretStore::new(ServiceBackend::new(disable_keyring))
                    .with_file_fallback(file_backend, consented),
            )
        });
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

    async fn lookup_background_secret(&self, setting_id_str: &str) -> Option<SecretString> {
        let store = self.secret_store.as_ref()?;
        let setting_id = SettingId::new(setting_id_str).ok()?;
        let secret_id = SecretId::from_setting_id(&setting_id).ok()?;
        let call = BrokerCall {
            interaction: FetchInteraction::Background,
            cancel: CancellationToken::new(),
            request_id: format!("hooks-watch-secret-{setting_id_str}"),
        };
        match store.background(call).lookup(&secret_id).await {
            SecretLookup::Found(secret) => Some(secret),
            _ => None,
        }
    }
}

impl HooksWatchObserver for EngineHooksWatchObserver {
    fn observe<'a>(
        &'a self,
        provider: &'a ProviderId,
        config: &'a Config,
        _verbose: bool,
        web_timeout: f64,
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

            let prefs = AppPreferences::from_config(config);
            let provider_cfg = config.provider_config(provider.as_str());
            let selected_account = ProviderRegistry::selected_account(provider_cfg);

            let api_key = self
                .lookup_background_secret(&format!("providers.{}.apiKey", provider.as_str()))
                .await;
            let manual_cookie_header = self
                .lookup_background_secret(&format!("providers.{}.cookieHeader", provider.as_str()))
                .await;
            let selected_account_token = if let Some(acct) = &selected_account {
                let tok = self
                    .lookup_background_secret(&format!(
                        "providers.{}.accounts.{}.token",
                        provider.as_str(),
                        acct.id
                    ))
                    .await;
                if tok.is_none() {
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
                tok
            } else {
                None
            };

            let inputs = ProviderPlanInputs {
                base_environment: self.base_environment.clone(),
                api_key,
                manual_cookie_header,
                selected_account_token,
                include_optional_usage: false,
                secret_store: self.secret_store.clone(),
            };

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

            let cancel = CancellationToken::new();
            let timeout_guard = if web_timeout.is_finite() && web_timeout > 0.0 {
                let token = cancel.clone();
                Some(tokio::spawn(async move {
                    tokio::time::sleep(Duration::from_secs_f64(web_timeout)).await;
                    token.cancel();
                }))
            } else {
                None
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
                cancel,
            };

            let sleeper = TokioSleeper;
            let outcome = fetch_outcome(provider, spec.plan.as_ref(), &cx, &sleeper).await;
            if let Some(guard) = timeout_guard {
                guard.abort();
            }

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
    executor: &dyn HookCommandExecutor,
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
                HooksErrorKind::Config,
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
    let web_timeout = decode_hooks_watch_web_timeout(args.web_timeout.as_deref());

    let mut detector = HookTransitionDetector::new();
    let mut rate_limiter = HookRateLimiter::default();

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
            let observation = observer
                .observe(provider, &config, verbose, web_timeout)
                .await;
            let now = Timestamp::now();
            let now = Timestamp::from_second(now.as_second()).unwrap_or(now);
            let dispatches = detector.evaluate(&observation, &hooks, now);
            for dispatch in dispatches {
                let dispatch_config = match dispatch.rules {
                    Some(rules) => HooksConfig::new(true, rules),
                    None => hooks.clone(),
                };
                let outcome = HookRunner::dispatch_with_executor(
                    executor,
                    &dispatch.event,
                    &dispatch_config,
                    &mut rate_limiter,
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
            let store_path = paths::config_path().unwrap_or_else(|_| Path::new("").to_path_buf());
            let store = ConfigStore::new(store_path);
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
            let executor = ProcessBrokerHookExecutor::new();
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
                    &executor,
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
    use ariadusage_core::refresh::{
        HOOKS_WATCH_SLEEP_TICK_NANOSECONDS, HookQuotaLaneKey, HookRule,
    };
    use ariadusage_engine::hooks::{FakeHookCommandExecutor, HookExecutionOutput, HookRunnerError};
    use ariadusage_protocol::Confidence;
    use clap::Parser;
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

    // upstream: Tests/CodexBarTests/CLIHooksTests.swift:8 watch provider option accepts all both and deduplicates concrete names
    #[test]
    fn watch_provider_option_accepts_all_both_and_deduplicates_concrete_names() {
        let all = decode_hooks_watch_provider_names(&["all"]).expect("all must succeed");
        let expected_all: Vec<ProviderId> = providers::first_party_order()
            .iter()
            .map(|d| d.id.clone())
            .collect();
        assert_eq!(all, Some(expected_all));

        let codex = ProviderId::new("codex").expect("valid provider id");
        let claude = ProviderId::new("claude").expect("valid provider id");
        let both = decode_hooks_watch_provider_names(&["both", "codex", "codex"])
            .expect("both+codex must succeed")
            .expect("non-empty provider list");
        assert_eq!(both, vec![codex, claude]);

        assert!(
            decode_hooks_watch_provider_names(&["bogus"]).is_err(),
            "Expected an unknown provider to fail"
        );
    }

    // upstream: Tests/CodexBarTests/CLIHooksTests.swift:27 watch privacy keeps account routing private and skips synthetic lanes
    #[test]
    fn watch_privacy_keeps_account_routing_private_and_skips_synthetic_lanes() {
        let now = Timestamp::from_second(1_700_000_000).expect("valid timestamp");
        let codex = ProviderId::new("codex").expect("valid provider id");
        let primary =
            RateWindow::new(0.0, Some(300), None, None, None, false).expect("valid primary");
        let secondary =
            RateWindow::new(0.0, None, None, None, None, true).expect("synthetic secondary");
        let identity = ProviderIdentitySnapshot {
            provider_id: Some(codex.clone()),
            account_id: None,
            account_email: Some("fixture@example.invalid".to_string()),
            account_organization: None,
            login_method: None,
        };
        let usage = sample_usage_snapshot(Some(primary), Some(secondary), Some(identity), now);

        let lanes = hooks_watch_lanes(
            &codex,
            &usage,
            &Config::new(Config::CURRENT_VERSION, Vec::new()),
            Some("private-owner"),
            true,
        );
        assert_eq!(lanes.len(), 1);
        assert_eq!(lanes[0].account_display_name, None);
        assert_eq!(
            lanes[0].key.account_discriminator.as_deref(),
            Some("private-owner")
        );
        assert_eq!(
            lanes[0].rate_window.as_ref().map(|w| w.used_percent),
            Some(0.0)
        );
    }

    // upstream: Tests/CodexBarTests/CLIHooksTests.swift:55 sample quota-low event matches maximum threshold
    #[test]
    fn sample_quota_low_event_matches_maximum_threshold() {
        let now = Timestamp::from_second(1_700_000_000).expect("valid timestamp");
        let event = sample_hook_event(HookEventType::QuotaLow, "codex", now);
        let rule = HookRule::new(HookEventType::QuotaLow, "/bin/echo").with_threshold(1.0);

        assert_eq!(event.usage_percent, Some(1.0));
        assert!(rule.matches(&event));
    }

    // upstream: Tests/CodexBarTests/CLIHooksTests.swift:64 sample refresh failure uses production status
    #[test]
    fn sample_refresh_failure_uses_production_status() {
        let now = Timestamp::from_second(1_700_000_000).expect("valid timestamp");
        let event = sample_hook_event(HookEventType::RefreshFailed, "codex", now);

        assert_eq!(event.status.as_deref(), Some("error"));
    }

    // upstream: Tests/CodexBarTests/CLIHooksTests.swift:71 sample usage update represents session and weekly windows
    #[test]
    fn sample_usage_update_represents_session_and_weekly_windows() {
        let now = Timestamp::from_second(1_700_000_000).expect("valid timestamp");
        let event = sample_hook_event(HookEventType::UsageUpdated, "codex", now);

        assert_eq!(event.usage_percent, Some(0.5));
        assert_eq!(event.window_minutes, Some(300));
        assert!(event.reset_at.is_some());
        assert_eq!(event.secondary_usage_percent, Some(0.4));
        assert_eq!(event.secondary_window_minutes, Some(10080));
        assert!(event.secondary_reset_at.is_some());
    }

    // upstream: Tests/CodexBarTests/CLIHooksTests.swift:83 usage updated event carries primary and secondary quota windows
    #[test]
    fn usage_updated_event_carries_primary_and_secondary_quota_windows() {
        let primary_reset: Timestamp = "2026-09-08T12:00:00Z".parse().expect("valid primary reset");
        let secondary_reset: Timestamp = "2026-09-10T12:00:00Z"
            .parse()
            .expect("valid secondary reset");
        let event = HookEvent {
            event: HookEventType::UsageUpdated,
            provider: "codex".to_string(),
            account: None,
            window: Some("Session".to_string()),
            usage_percent: Some(0.2),
            window_minutes: None,
            used: None,
            limit: None,
            reset_at: Some(primary_reset),
            secondary_usage_percent: Some(0.6),
            secondary_window_minutes: None,
            secondary_reset_at: Some(secondary_reset),
            status: None,
            timestamp: primary_reset,
        };

        let data = event.json_payload().expect("json payload should encode");
        let decoded: HookEvent = serde_json::from_slice(&data).expect("json payload should decode");

        assert_eq!(decoded.event, HookEventType::UsageUpdated);
        assert_eq!(decoded.usage_percent, Some(0.2));
        assert_eq!(decoded.secondary_usage_percent, Some(0.6));
        assert_eq!(decoded.secondary_reset_at, Some(secondary_reset));
    }

    // upstream: Tests/CodexBarTests/CLIHooksTests.swift:108 hook test JSON result is structured
    #[test]
    fn hook_test_json_result_is_structured() {
        let result = HookTestResult {
            rule_id: "fixture".to_string(),
            executable: "/bin/echo".to_string(),
            event: "quota_reached".to_string(),
            provider: "codex".to_string(),
            success: true,
            stdout: Some("ok".to_string()),
            error: None,
        };
        let encoded =
            encode_json(std::slice::from_ref(&result), false).expect("encode HookTestResult");
        let decoded: Vec<HookTestResult> =
            serde_json::from_str(&encoded).expect("decode HookTestResult");

        assert_eq!(decoded, vec![result]);
    }

    // upstream: TestsLinux/CLIHooksWatchSleepLinuxTests.swift:7 already requested stop skips every sleep tick
    #[tokio::test]
    async fn already_requested_stop_skips_every_sleep_tick() {
        let stop = HooksWatchStopSignal::new();
        stop.request();

        sleep_interruptibly(30.0, &stop, |_nanoseconds| {
            panic!("An already requested stop must not sleep");
            #[allow(unreachable_code)]
            async {}
        })
        .await;
    }

    // upstream: TestsLinux/CLIHooksWatchSleepLinuxTests.swift:17 stop requested during a tick prevents the next sleep
    #[tokio::test]
    async fn stop_requested_during_a_tick_prevents_the_next_sleep() {
        let stop = HooksWatchStopSignal::new();
        let mut ticks = Vec::new();
        sleep_interruptibly(10.0, &stop, |nanoseconds| {
            ticks.push(nanoseconds);
            stop.request();
            async {}
        })
        .await;

        assert_eq!(ticks, vec![200_000_000]);
    }

    // upstream: TestsLinux/CLIHooksWatchSleepLinuxTests.swift:31 unsignaled sleep requests the full interval in bounded ticks
    #[tokio::test]
    async fn unsignaled_sleep_requests_the_full_interval_in_bounded_ticks() {
        for interval in [0.0, -1.0, 0.05, 0.4, 0.45] {
            let stop = HooksWatchStopSignal::new();
            let mut ticks = Vec::new();
            sleep_interruptibly(interval, &stop, |nanoseconds| {
                ticks.push(nanoseconds);
                async {}
            })
            .await;

            let expected = (interval.max(0.0) * 1_000_000_000.0).round() as u64;
            assert_eq!(ticks.iter().copied().sum::<u64>(), expected);
            assert!(
                ticks
                    .iter()
                    .all(|&t| t > 0 && t <= HOOKS_WATCH_SLEEP_TICK_NANOSECONDS)
            );
            assert_eq!(ticks.len(), expected.div_ceil(200_000_000) as usize);
        }
    }

    // upstream: TestsLinux/CLIHooksWatchArgumentPrecedenceLinuxTests.swift:23 once mode is rejected because watch state must survive between polls
    #[test]
    fn once_mode_is_rejected_because_watch_state_must_survive_between_polls() {
        let err = validate_hooks_watch_flags(true).expect_err("expected --once to be rejected");
        assert!(err.message.contains("--once"));
    }

    // upstream: TestsLinux/CLIHooksWatchArgumentPrecedenceLinuxTests.swift:35 interval below the floor is rejected without reading config
    #[test]
    fn interval_below_the_floor_is_rejected_without_reading_config() {
        let err = decode_hooks_watch_interval(Some("5"))
            .expect_err("expected failure for an interval below the floor");
        assert!(err.message.contains("at least 60"));
    }

    // upstream: TestsLinux/CLIHooksWatchArgumentPrecedenceLinuxTests.swift:45 non numeric interval is rejected without reading config
    #[test]
    fn non_numeric_interval_is_rejected_without_reading_config() {
        let err = decode_hooks_watch_interval(Some("abc"))
            .expect_err("expected failure for a non-numeric interval");
        assert!(err.message.contains("Invalid --interval"));
    }

    // upstream: TestsLinux/CLIHooksWatchArgumentPrecedenceLinuxTests.swift:55 interval defaults when absent
    #[test]
    fn interval_defaults_when_absent() {
        let interval = decode_hooks_watch_interval(None).expect("expected the default interval");
        assert_eq!(
            interval,
            ariadusage_core::refresh::HOOKS_WATCH_DEFAULT_INTERVAL
        );
    }

    // upstream: TestsLinux/CLIHooksWatchArgumentPrecedenceLinuxTests.swift:65 interval at the floor is accepted
    #[test]
    fn interval_at_the_floor_is_accepted() {
        let interval = decode_hooks_watch_interval(Some("60"))
            .expect("expected the floor value to be accepted");
        assert_eq!(interval, 60.0);
    }

    // upstream: TestsLinux/CLIHooksWatchArgumentPrecedenceLinuxTests.swift:77 unknown provider name is rejected without reading config
    #[test]
    fn unknown_provider_name_is_rejected_without_reading_config() {
        let err = decode_hooks_watch_provider_names(&["nosuchprovider"])
            .expect_err("expected failure for an unknown provider");
        assert!(err.message.contains("Unknown provider"));
    }

    // upstream: TestsLinux/CLIHooksWatchArgumentPrecedenceLinuxTests.swift:88 absent provider option defers to config
    #[test]
    fn absent_provider_option_defers_to_config() {
        let providers = decode_hooks_watch_provider_names(&[])
            .expect("expected success when no --provider is given");
        assert_eq!(providers, None);
    }

    // upstream: TestsLinux/CLIHooksWatchArgumentPrecedenceLinuxTests.swift:99 explicit providers resolve without consulting config
    #[test]
    fn explicit_providers_resolve_without_consulting_config() {
        let codex = ProviderId::new("codex").expect("valid id");
        let providers = decode_hooks_watch_provider_names(&["codex", "codex"])
            .expect("expected success for a known provider");
        assert_eq!(providers, Some(vec![codex]));
    }

    // upstream: TestsLinux/CLIHooksWatchArgumentPrecedenceLinuxTests.swift:113 explicit providers win over the enabled set
    #[test]
    fn explicit_providers_win_over_the_enabled_set() {
        let codex = ProviderId::new("codex").expect("valid id");
        let config = Config::new(Config::CURRENT_VERSION, Vec::new());
        let providers = hooks_watch_providers(Some(vec![codex.clone()]), &config)
            .expect("expected explicit providers to resolve");
        assert_eq!(providers, vec![codex]);
    }

    // upstream: TestsLinux/CLIHooksWatchArgumentPrecedenceLinuxTests.swift:124 empty enabled set reports no providers
    #[test]
    fn empty_enabled_set_reports_no_providers() {
        let mut entries = Vec::new();
        for desc in providers::first_party_order() {
            let mut p = ProviderConfig::new(desc.id.clone());
            p.enabled = Some(false);
            entries.push(ProviderEntry::Typed(p));
        }
        let config = Config::new(Config::CURRENT_VERSION, entries);
        let err = hooks_watch_providers(None, &config)
            .expect_err("expected failure when nothing is enabled");
        assert!(err.message.contains("No providers are enabled"));
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
        let test_parsed = TestHooksCli::try_parse_from([
            "ariadusage",
            "hooks",
            "test",
            "quota_reached",
            "--format",
            "json",
            "--json",
            "--json-only",
            "--pretty",
        ])
        .expect("hooks test output flags must parse");
        match test_parsed.command {
            TestTopCommand::Hooks(HooksCommandArgs {
                subcommand: Some(HooksSubcommand::Test(args)),
                ..
            }) => {
                assert_eq!(args.event.as_deref(), Some("quota_reached"));
                assert_eq!(args.output.format.as_deref(), Some("json"));
                assert!(args.output.json);
                assert!(args.output.json_only);
                assert!(args.output.pretty);
            }
            other => panic!("unexpected subcommand: {other:?}"),
        }

        let watch_parsed = TestHooksCli::try_parse_from([
            "ariadusage",
            "hooks",
            "watch",
            "--format",
            "json",
            "--json",
            "--json-only",
            "--pretty",
        ])
        .expect("hooks watch output flags must parse");
        match watch_parsed.command {
            TestTopCommand::Hooks(HooksCommandArgs {
                subcommand: Some(HooksSubcommand::Watch(args)),
                ..
            }) => {
                assert_eq!(args.output.format.as_deref(), Some("json"));
                assert!(args.output.json);
                assert!(args.output.json_only);
                assert!(args.output.pretty);
            }
            other => panic!("unexpected subcommand: {other:?}"),
        }
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
            _web_timeout: f64,
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
        let executor = FakeHookCommandExecutor::new();
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
            &executor,
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
