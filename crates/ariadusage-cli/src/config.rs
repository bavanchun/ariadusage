// Ported from CodexBar Sources/CodexBarCLI/CLIConfigCommand.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBarCLI/CLIConfigSourceCommand.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

//! CLI `config` command (`validate`, `dump`, `providers`, `enable`, `disable`, `set-source`).
//!
//! Security invariants:
//! - Secrets belong exclusively in `SecretStore` (managed via `ariadusage secret set`).
//! - No `ariadusage config` subcommand ever prints a secret value or writes a secret to `config.json`.
//! - `ariadusage config dump` redacts all legacy credential fields (`[REDACTED]`) and rejects
//!   `--show-secrets` with exit code `20`.
//! - `ariadusage config set-api-key` is rejected with exit code `20` and directs the user to
//!   `ariadusage secret set` without echoing any supplied value.

use std::io::Write;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

use ariadusage_core::config::{
    Config, Issue, IssueSeverity, ProviderConfig, ProviderEntry, SecretPresence,
    sanitized_for_dump, set_provider_enabled, validate,
};
use ariadusage_core::pipeline::FetchInteraction;
use ariadusage_core::providers::{self, ProviderDescriptor, SourceMode};
use ariadusage_engine::brokers::call::BrokerCall;
use ariadusage_engine::build_default_secret_store;
use ariadusage_engine::config_store::ConfigStore;
use ariadusage_engine::error::StoreError;
use ariadusage_engine::paths;
use ariadusage_protocol::{
    ConfigIssueSeverity, ConfigProviderStatusResult, ConfigProviderToggleResult,
    ConfigSetSourceResult, ConfigValidationIssuePayload, ProviderId,
};
use clap::{Args, Subcommand};
use tokio_util::sync::CancellationToken;

pub use crate::hooks::HooksOutputFormat as OutputFormat;
use crate::hooks::{
    HooksCliError, HooksErrorKind, HooksOutputArgs, HooksOutputFormat, HooksOutputPreferences,
};

/// CLI log level (`CodexBarLog.Level` in `CodexBarLog.swift`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum CliLogLevel {
    Trace,
    Verbose,
    Debug,
    Info,
    Warning,
    Error,
    Critical,
}

impl CliLogLevel {
    /// Parses a raw `--log-level` value (`trace|verbose|debug|info|warning|error|critical`).
    #[must_use]
    pub fn parse_level(raw: Option<&str>) -> Option<Self> {
        let trimmed = raw?.trim();
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

    /// Resolves the effective log level from `--verbose` and `--log-level` (`CodexBarCLI.resolvedLogLevel`).
    #[must_use]
    pub fn resolved(verbose: bool, raw_level: Option<&str>) -> Self {
        Self::parse_level(raw_level).unwrap_or(if verbose { Self::Debug } else { Self::Error })
    }
}

/// Resolved CLI logging configuration (`CodexBarLog.Configuration` in `CLIEntry.swift`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CliLoggingConfiguration {
    pub level: CliLogLevel,
    pub json: bool,
}

impl CliLoggingConfiguration {
    /// Builds the CLI logging configuration from CLI flags (`CodexBarCLI.loggingConfiguration`).
    #[must_use]
    pub fn from_flags(
        verbose: bool,
        json_output: bool,
        json_only: bool,
        log_level: Option<&str>,
    ) -> Self {
        Self {
            level: CliLogLevel::resolved(verbose, log_level),
            json: json_output || json_only,
        }
    }
}

static CLI_LOGGING_CONFIG: OnceLock<Mutex<Option<CliLoggingConfiguration>>> = OnceLock::new();

/// Bootstraps or updates the process-wide CLI logging configuration (`CodexBarLog.bootstrapIfNeeded`).
pub fn bootstrap_cli_logging(config: CliLoggingConfiguration) {
    let cell = CLI_LOGGING_CONFIG.get_or_init(|| Mutex::new(None));
    if let Ok(mut guard) = cell.lock() {
        *guard = Some(config);
    }
}

/// Returns the most recently bootstrapped CLI logging configuration.
#[must_use]
pub fn current_cli_logging_configuration() -> Option<CliLoggingConfiguration> {
    CLI_LOGGING_CONFIG
        .get()
        .and_then(|cell| cell.lock().ok().and_then(|g| *g))
}

/// Exit code for invalid CLI arguments (`CLIExitCode.invalidArguments`).
pub const EXIT_ARGS: i32 = 2;

/// Parses `--format`, `--json`, and `--json-only` into an [`OutputFormat`].
pub fn parse_output_format(
    format: Option<&str>,
    json: bool,
    json_only: bool,
) -> Result<OutputFormat, String> {
    if json || json_only {
        return Ok(OutputFormat::Json);
    }
    let Some(raw) = format.map(str::trim).filter(|s| !s.is_empty()) else {
        return Ok(OutputFormat::Text);
    };
    match raw.to_ascii_lowercase().as_str() {
        "text" => Ok(OutputFormat::Text),
        "json" => Ok(OutputFormat::Json),
        _ => Err(format!("Unsupported --format '{raw}'. Use text or json.")),
    }
}

/// Exit code for unsupported `config` operations (`config set-api-key`, `config dump --show-secrets`,
/// `config preferences`). Does not reuse exit code `17`.
pub const UNSUPPORTED_CONFIG_OPERATION_EXIT_CODE: u8 = 20;

/// Extended `--help` description for `ariadusage config` (`CLIHelp.configHelp`).
pub const CONFIG_LONG_ABOUT: &str = "\
Validate, inspect, or update the AriadUsage config file (default: validate).
dump prints normalized config JSON with any legacy credentials redacted as [REDACTED].
providers lists persistent provider enablement for v1 providers and preserved entries.
enable/disable updates persistent provider enablement in config.json.
set-source stores a supported data source without changing provider enablement; auto clears the override.
Secrets are managed exclusively with 'ariadusage secret set' and are never printed by config commands.

Examples:
  ariadusage config validate --format json --pretty
  ariadusage config dump --pretty
  ariadusage config providers
  ariadusage config enable --provider claude
  ariadusage config disable --provider antigravity
  ariadusage config set-source --provider claude --source cli";

/// Common output and logging flags accepted by `config` subcommands (`CLICommonOptions`).
#[derive(Args, Debug, Clone, Default, PartialEq, Eq)]
pub struct ConfigCommonArgs {
    #[command(flatten)]
    pub output: HooksOutputArgs,

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

impl ConfigCommonArgs {
    /// Returns the resolved [`CliLoggingConfiguration`] for these flags.
    #[must_use]
    pub fn logging_configuration(&self) -> CliLoggingConfiguration {
        CliLoggingConfiguration::from_flags(
            self.verbose,
            self.json_output,
            self.output.json_only,
            self.log_level.as_deref(),
        )
    }
}

/// Top-level `ariadusage config` arguments.
#[derive(Args, Debug, Clone, Default)]
pub struct ConfigCommandArgs {
    #[command(flatten)]
    pub common: ConfigCommonArgs,

    #[command(subcommand)]
    pub subcommand: Option<ConfigSubcommand>,
}

/// Subcommands under `ariadusage config`.
#[derive(Subcommand, Debug, Clone)]
pub enum ConfigSubcommand {
    /// Validate the configuration file
    Validate(ConfigCommonArgs),

    /// Print normalized configuration JSON with credentials redacted
    Dump(ConfigDumpArgs),

    /// List configured providers and their enablement state
    Providers(ConfigCommonArgs),

    /// Enable a provider in configuration
    Enable(ConfigProviderToggleArgs),

    /// Disable a provider in configuration
    Disable(ConfigProviderToggleArgs),

    /// Set or clear the persistent data source for a provider
    SetSource(ConfigSetSourceArgs),

    /// Unsupported in AriadUsage; directs users to `ariadusage secret set`
    SetApiKey(ConfigSetApiKeyArgs),

    /// Unsupported macOS app preferences bridge
    Preferences(ConfigPreferencesArgs),
}

/// Arguments for `ariadusage config dump` (`ConfigDumpOptions`).
#[derive(Args, Debug, Clone, Default, PartialEq, Eq)]
pub struct ConfigDumpArgs {
    /// Unsupported in AriadUsage; rejected to prevent printing secrets
    #[arg(long = "show-secrets")]
    pub show_secrets: bool,

    #[command(flatten)]
    pub common: ConfigCommonArgs,
}

/// Arguments for `ariadusage config enable` and `ariadusage config disable` (`ConfigProviderToggleOptions`).
#[derive(Args, Debug, Clone, Default, PartialEq, Eq)]
pub struct ConfigProviderToggleArgs {
    /// Provider name: codex | claude | antigravity
    #[arg(long = "provider")]
    pub provider: Option<String>,

    #[command(flatten)]
    pub common: ConfigCommonArgs,
}

/// Arguments for `ariadusage config set-source` (`ConfigSetSourceOptions`).
#[derive(Args, Debug, Clone, Default, PartialEq, Eq)]
pub struct ConfigSetSourceArgs {
    /// Provider name: codex | claude | antigravity
    #[arg(long = "provider")]
    pub provider: Option<String>,

    /// Persistent source: auto, web, cli, oauth, or api
    #[arg(long = "source")]
    pub source: Option<String>,

    #[command(flatten)]
    pub common: ConfigCommonArgs,
}

/// Arguments for `ariadusage config set-api-key` (`ConfigSetAPIKeyOptions`).
#[derive(Args, Debug, Clone, Default, PartialEq, Eq)]
pub struct ConfigSetApiKeyArgs {
    /// Provider name
    #[arg(long = "provider")]
    pub provider: Option<String>,

    /// API key (rejected; never stored in config.json or echoed)
    #[arg(long = "api-key")]
    pub api_key: Option<String>,

    /// Read API key from stdin
    #[arg(long = "stdin")]
    pub stdin: bool,

    /// Store the key without enabling the provider
    #[arg(long = "no-enable")]
    pub no_enable: bool,

    /// Token-account label
    #[arg(long = "label")]
    pub label: Option<String>,

    /// Token-account usage scope
    #[arg(long = "usage-scope")]
    pub usage_scope: Option<String>,

    /// Organization ID
    #[arg(long = "organization-id")]
    pub organization_id: Option<String>,

    /// Workspace ID
    #[arg(long = "workspace-id")]
    pub workspace_id: Option<String>,

    #[command(flatten)]
    pub common: ConfigCommonArgs,
}

/// Arguments for `ariadusage config preferences`.
#[derive(Args, Debug, Clone, Default, PartialEq, Eq)]
pub struct ConfigPreferencesArgs {
    /// Subcommand action (`export` or `import`)
    pub action: Option<String>,

    /// Preferences file path
    #[arg(long = "file")]
    pub file: Option<PathBuf>,

    /// Defaults domain
    #[arg(long = "defaults-domain")]
    pub defaults_domain: Option<String>,

    #[command(flatten)]
    pub common: ConfigCommonArgs,
}

/// Returns the canonical lowercase CLI string for a [`SourceMode`].
#[must_use]
pub const fn source_mode_str(mode: SourceMode) -> &'static str {
    match mode {
        SourceMode::Auto => "auto",
        SourceMode::Web => "web",
        SourceMode::Cli => "cli",
        SourceMode::Oauth => "oauth",
        SourceMode::Api => "api",
    }
}

/// Parses a lowercase or mixed-case [`SourceMode`] string (`auto`, `web`, `cli`, `oauth`, `api`).
#[must_use]
pub fn parse_source_mode(raw: &str) -> Option<SourceMode> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "auto" => Some(SourceMode::Auto),
        "web" => Some(SourceMode::Web),
        "cli" => Some(SourceMode::Cli),
        "oauth" => Some(SourceMode::Oauth),
        "api" => Some(SourceMode::Api),
        _ => None,
    }
}

/// Resolves a v1 provider descriptor by CLI name or ID (case-insensitive).
#[must_use]
pub fn resolve_config_provider(raw: Option<&str>) -> Option<&'static ProviderDescriptor> {
    let trimmed = raw?.trim();
    if trimmed.is_empty() {
        return None;
    }
    let lowered = trimmed.to_ascii_lowercase();
    providers::first_party_order()
        .iter()
        .find(|d| d.id.as_str() == lowered || d.cli_name.eq_ignore_ascii_case(&lowered))
}

/// Validates `--provider` and `--source` for `config set-source` (`CodexBarCLI.configSourceSelection`).
pub fn config_source_selection(
    raw_provider: Option<&str>,
    raw_source: Option<&str>,
) -> Result<(&'static ProviderDescriptor, SourceMode), String> {
    let Some(descriptor) = resolve_config_provider(raw_provider) else {
        return Err("Unknown or missing provider. Use --provider <name>.".to_string());
    };
    let Some(source) = raw_source.and_then(parse_source_mode) else {
        return Err("Unknown or missing source. Use --source auto|web|cli|oauth|api.".to_string());
    };
    if !descriptor.source_modes.contains(&source) {
        let mut choices: Vec<&str> = descriptor
            .source_modes
            .iter()
            .map(|m| source_mode_str(*m))
            .collect();
        choices.sort_unstable();
        return Err(format!(
            "Source {} is not supported for {}. Supported sources: {}.",
            source_mode_str(source),
            descriptor.id.as_str(),
            choices.join(", ")
        ));
    }
    Ok((descriptor, source))
}

/// Updates `config` with the selected `source` for `provider` (`CodexBarCLI.configSettingSource`).
///
/// Setting `SourceMode::Auto` clears the explicit `source` override (`None`) while preserving all
/// other fields on the provider and any unknown/opaque provider entries.
#[must_use]
pub fn config_setting_source(config: &Config, provider: &ProviderId, source: SourceMode) -> Config {
    let mut updated = ariadusage_core::config::normalize(config.clone());
    let mut entry = updated
        .provider_config(provider.as_str())
        .cloned()
        .unwrap_or_else(|| ProviderConfig::new(provider.clone()));
    entry.source = if source == SourceMode::Auto {
        None
    } else {
        Some(source)
    };
    updated.set_provider_config(entry);
    updated
}

/// Updates `config` with the selected `enabled` state for `provider` (`CodexBarCLI.configSettingProviderEnabled`).
#[must_use]
pub fn config_setting_provider_enabled(
    config: &Config,
    provider: &ProviderId,
    enabled: bool,
) -> Config {
    let mut updated = ariadusage_core::config::normalize(config.clone());
    set_provider_enabled(&mut updated, provider.as_str(), enabled);
    updated
}

/// Computes the ordered provider status list for `config providers` (`CodexBarCLI.configProviderStatuses`).
#[must_use]
pub fn config_provider_statuses(config: &Config) -> Vec<ConfigProviderStatusResult> {
    let normalized = ariadusage_core::config::normalize(config.clone());
    normalized
        .providers
        .iter()
        .map(|entry| match entry {
            ProviderEntry::Typed(provider_cfg) => {
                let descriptor = providers::find_by_id(&provider_cfg.id);
                let default_enabled = descriptor.is_some_and(|d| d.default_enabled);
                let display_name = descriptor.map_or_else(
                    || provider_cfg.id.as_str().to_string(),
                    |d| d.display_name.to_string(),
                );
                let enabled = provider_cfg.enabled.unwrap_or(default_enabled);
                ConfigProviderStatusResult {
                    provider: provider_cfg.id.as_str().to_string(),
                    display_name,
                    enabled,
                    default_enabled,
                }
            }
            ProviderEntry::Opaque { id, enabled, .. } => {
                if let Some(descriptor) = providers::find_by_id_str(id) {
                    ConfigProviderStatusResult {
                        provider: id.clone(),
                        display_name: descriptor.display_name.to_string(),
                        enabled: *enabled,
                        default_enabled: descriptor.default_enabled,
                    }
                } else {
                    ConfigProviderStatusResult {
                        provider: id.clone(),
                        display_name: "plugin (not loaded)".to_string(),
                        enabled: *enabled,
                        default_enabled: false,
                    }
                }
            }
        })
        .collect()
}

/// Renders human-readable output for `config validate` (`CodexBarCLI.runConfigValidate`).
#[must_use]
pub fn render_config_validate_text(issues: &[Issue]) -> String {
    if issues.is_empty() {
        return "Config: OK".to_string();
    }
    issues
        .iter()
        .map(|issue| {
            let provider = issue.provider.as_ref().map_or("config", ProviderId::as_str);
            let field = issue.field.as_deref().unwrap_or("");
            let prefix = match issue.severity {
                IssueSeverity::Warning => "[WARNING]",
                IssueSeverity::Error => "[ERROR]",
            };
            let suffix = if field.is_empty() {
                String::new()
            } else {
                format!(" ({field})")
            };
            format!("{prefix} {provider}{suffix}: {}", issue.message)
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Renders human-readable output for `config providers` (`CodexBarCLI.runConfigProviders`).
#[must_use]
pub fn render_config_providers_text(results: &[ConfigProviderStatusResult]) -> String {
    results
        .iter()
        .map(|result| {
            let state = if result.enabled {
                "enabled"
            } else {
                "disabled"
            };
            let marker = if result.default_enabled {
                " default"
            } else {
                ""
            };
            format!(
                "{}: {state}{marker} ({})",
                result.provider, result.display_name
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Renders human-readable output for `config enable` / `config disable` (`CodexBarCLI.runConfigSetProviderEnabled`).
#[must_use]
pub fn render_config_provider_toggle_text(result: &ConfigProviderToggleResult) -> String {
    let state = if result.enabled {
        "enabled"
    } else {
        "disabled"
    };
    format!("Config: {state} {}", result.display_name)
}

/// Renders human-readable output for `config set-source` (`CodexBarCLI.runConfigSetSource`).
#[must_use]
pub fn render_config_set_source_text(result: &ConfigSetSourceResult) -> String {
    format!(
        "Config: {} source set to {}",
        result.display_name, result.source
    )
}

fn args_failure(message: impl Into<String>, output: HooksOutputPreferences) -> HooksCliError {
    HooksCliError {
        code: EXIT_ARGS as u8,
        message: Some(message.into()),
        output,
        kind: HooksErrorKind::Args,
    }
}

fn resolve_config_output(args: &HooksOutputArgs) -> Result<HooksOutputPreferences, HooksCliError> {
    let fallback = HooksOutputPreferences::from_args(args);
    let format = parse_output_format(args.format.as_deref(), args.json, args.json_only)
        .map_err(|msg| args_failure(msg, fallback))?;
    Ok(HooksOutputPreferences {
        format,
        json_only: args.json_only,
        pretty: args.pretty,
    })
}

fn default_config_store(output: HooksOutputPreferences) -> Result<ConfigStore, HooksCliError> {
    let path = paths::config_path()
        .map_err(|err| HooksCliError::failure(err.to_string(), output, HooksErrorKind::Config))?;
    Ok(ConfigStore::new(path))
}

fn format_store_error(error: &StoreError) -> String {
    match error {
        StoreError::Decode(err) => format!("Failed to decode AriadUsage config: {err}"),
        other => other.to_string(),
    }
}

fn load_config_from_store(
    store: &ConfigStore,
    output: HooksOutputPreferences,
) -> Result<Config, HooksCliError> {
    store.load_effective().map_err(|err| {
        HooksCliError::failure(format_store_error(&err), output, HooksErrorKind::Config)
    })
}

fn save_config_to_store(
    store: &ConfigStore,
    config: &Config,
    output: HooksOutputPreferences,
) -> Result<(), HooksCliError> {
    store.save(config).map_err(|err| {
        HooksCliError::failure(format_store_error(&err), output, HooksErrorKind::Config)
    })
}

fn collect_secret_presence(config: &Config) -> SecretPresence {
    let mut presence = SecretPresence::none();

    for entry in &config.providers {
        let ProviderEntry::Typed(cfg) = entry else {
            continue;
        };
        if cfg
            .extra
            .get("apiKey")
            .and_then(|v| serde_json::from_str::<String>(v.get()).ok())
            .is_some_and(|s| !s.trim().is_empty())
        {
            presence.api_keys.insert(cfg.id.clone());
        }
        if cfg
            .extra
            .get("cookieHeader")
            .and_then(|v| serde_json::from_str::<String>(v.get()).ok())
            .is_some_and(|s| !s.trim().is_empty())
        {
            presence.cookie_headers.insert(cfg.id.clone());
        }
        if let Some(ta) = &cfg.token_accounts {
            for acc in &ta.accounts {
                if acc
                    .extra
                    .get("token")
                    .and_then(|v| serde_json::from_str::<String>(v.get()).ok())
                    .is_some_and(|s| !s.trim().is_empty())
                {
                    presence
                        .token_account_tokens
                        .insert((cfg.id.clone(), acc.id.clone()));
                }
            }
        }
    }

    let disable_keyring = std::env::var_os("ARIADUSAGE_DISABLE_KEYRING").as_deref()
        == Some(std::ffi::OsStr::new("1"));
    let Ok(secret_store) = build_default_secret_store(config, disable_keyring) else {
        return presence;
    };
    let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    else {
        return presence;
    };
    let call = BrokerCall {
        interaction: FetchInteraction::Background,
        cancel: CancellationToken::new(),
        request_id: "config-validate".to_string(),
    };

    let store_presence = runtime.block_on(secret_store.presence(config, call));
    presence.api_keys.extend(store_presence.api_keys);
    presence
        .cookie_headers
        .extend(store_presence.cookie_headers);
    presence
        .token_account_tokens
        .extend(store_presence.token_account_tokens);

    presence
}

fn print_json<T: serde::Serialize + ?Sized>(
    value: &T,
    pretty: bool,
    stdout: &mut dyn Write,
) -> std::io::Result<()> {
    let encoded = if pretty {
        serde_json::to_string_pretty(value).ok()
    } else {
        serde_json::to_string(value).ok()
    };
    if let Some(text) = encoded {
        writeln!(stdout, "{text}")?;
    }
    Ok(())
}

/// Runs `ariadusage config` against the default `ConfigStore` and process stdout.
pub fn run_config_command(args: ConfigCommandArgs) -> Result<(), HooksCliError> {
    let mut stdout = std::io::stdout();
    let effective_output = match &args.subcommand {
        Some(ConfigSubcommand::Validate(sub)) | Some(ConfigSubcommand::Providers(sub)) => {
            HooksOutputPreferences::from_args(&sub.output)
        }
        Some(ConfigSubcommand::Dump(sub)) => HooksOutputPreferences::from_args(&sub.common.output),
        Some(ConfigSubcommand::Enable(sub)) | Some(ConfigSubcommand::Disable(sub)) => {
            HooksOutputPreferences::from_args(&sub.common.output)
        }
        Some(ConfigSubcommand::SetSource(sub)) => {
            HooksOutputPreferences::from_args(&sub.common.output)
        }
        Some(ConfigSubcommand::SetApiKey(sub)) => {
            HooksOutputPreferences::from_args(&sub.common.output)
        }
        Some(ConfigSubcommand::Preferences(sub)) => {
            HooksOutputPreferences::from_args(&sub.common.output)
        }
        None => HooksOutputPreferences::from_args(&args.common.output),
    };
    let store = default_config_store(effective_output)?;
    run_config_command_with_store(args, &store, &mut stdout)
}

/// Runs `ariadusage config` against an injected [`ConfigStore`] and writer.
pub fn run_config_command_with_store(
    args: ConfigCommandArgs,
    store: &ConfigStore,
    stdout: &mut dyn Write,
) -> Result<(), HooksCliError> {
    match args.subcommand {
        None => run_config_validate_with_store(&args.common, store, stdout),
        Some(ConfigSubcommand::Validate(sub)) => {
            run_config_validate_with_store(&sub, store, stdout)
        }
        Some(ConfigSubcommand::Dump(sub)) => run_config_dump_with_store(&sub, store, stdout),
        Some(ConfigSubcommand::Providers(sub)) => {
            run_config_providers_with_store(&sub, store, stdout)
        }
        Some(ConfigSubcommand::Enable(sub)) => {
            run_config_set_provider_enabled_with_store(&sub, true, store, stdout)
        }
        Some(ConfigSubcommand::Disable(sub)) => {
            run_config_set_provider_enabled_with_store(&sub, false, store, stdout)
        }
        Some(ConfigSubcommand::SetSource(sub)) => {
            run_config_set_source_with_store(&sub, store, stdout)
        }
        Some(ConfigSubcommand::SetApiKey(sub)) => {
            bootstrap_cli_logging(sub.common.logging_configuration());
            let output = HooksOutputPreferences::from_args(&sub.common.output);
            Err(HooksCliError {
                code: UNSUPPORTED_CONFIG_OPERATION_EXIT_CODE,
                message: Some(
                    "Storing secrets in config.json via 'config set-api-key' is unsupported; use 'ariadusage secret set --id providers.<provider>.apiKey' instead."
                        .to_string(),
                ),
                output,
                kind: HooksErrorKind::Config,
            })
        }
        Some(ConfigSubcommand::Preferences(sub)) => {
            bootstrap_cli_logging(sub.common.logging_configuration());
            let output = HooksOutputPreferences::from_args(&sub.common.output);
            Err(HooksCliError {
                code: UNSUPPORTED_CONFIG_OPERATION_EXIT_CODE,
                message: Some(
                    "macOS app preferences export/import is unsupported in ariadusage CLI."
                        .to_string(),
                ),
                output,
                kind: HooksErrorKind::Config,
            })
        }
    }
}

/// Runs `ariadusage config validate` against `store`.
pub fn run_config_validate_with_store(
    common: &ConfigCommonArgs,
    store: &ConfigStore,
    stdout: &mut dyn Write,
) -> Result<(), HooksCliError> {
    bootstrap_cli_logging(common.logging_configuration());
    let output = resolve_config_output(&common.output)?;
    let config = load_config_from_store(store, output)?;
    let secrets = collect_secret_presence(&config);
    let issues = validate(&config, &secrets);
    let has_errors = issues
        .iter()
        .any(|issue| issue.severity == IssueSeverity::Error);

    match output.format {
        HooksOutputFormat::Text => {
            let _ = writeln!(stdout, "{}", render_config_validate_text(&issues));
        }
        HooksOutputFormat::Json => {
            let payload: Vec<ConfigValidationIssuePayload> = issues
                .iter()
                .map(|issue| ConfigValidationIssuePayload {
                    severity: match issue.severity {
                        IssueSeverity::Warning => ConfigIssueSeverity::Warning,
                        IssueSeverity::Error => ConfigIssueSeverity::Error,
                    },
                    provider: issue.provider.clone(),
                    field: issue.field.clone(),
                    code: issue.code.clone(),
                    message: issue.message.clone(),
                })
                .collect();
            let _ = print_json(&payload, output.pretty, stdout);
        }
    }

    if has_errors {
        Err(HooksCliError::silent_failure(
            output,
            HooksErrorKind::Config,
        ))
    } else {
        Ok(())
    }
}

/// Runs `ariadusage config dump` against `store`.
///
/// Rejects `--show-secrets` with [`UNSUPPORTED_CONFIG_OPERATION_EXIT_CODE`] so no secret value
/// is ever printed by a config command.
pub fn run_config_dump_with_store(
    args: &ConfigDumpArgs,
    store: &ConfigStore,
    stdout: &mut dyn Write,
) -> Result<(), HooksCliError> {
    bootstrap_cli_logging(args.common.logging_configuration());
    let output = resolve_config_output(&args.common.output)?;
    if args.show_secrets {
        return Err(HooksCliError {
            code: UNSUPPORTED_CONFIG_OPERATION_EXIT_CODE,
            message: Some(
                "Revealing secrets via 'config dump --show-secrets' is unsupported; secrets are managed by 'ariadusage secret set' and never printed."
                    .to_string(),
            ),
            output,
            kind: HooksErrorKind::Config,
        });
    }

    let config = load_config_from_store(store, output)?;
    let redacted = sanitized_for_dump(&config, false);
    let pretty_bytes = ariadusage_core::config::encode(&redacted);
    let mut bytes = if output.pretty {
        pretty_bytes
    } else {
        serde_json::from_slice::<serde_json::Value>(&pretty_bytes)
            .and_then(|value| serde_json::to_vec(&value))
            .map_err(|err| {
                HooksCliError::failure(err.to_string(), output, HooksErrorKind::Config)
            })?
    };
    bytes.push(b'\n');
    stdout
        .write_all(&bytes)
        .map_err(|err| HooksCliError::failure(err.to_string(), output, HooksErrorKind::Config))?;
    Ok(())
}

/// Runs `ariadusage config providers` against `store`.
pub fn run_config_providers_with_store(
    common: &ConfigCommonArgs,
    store: &ConfigStore,
    stdout: &mut dyn Write,
) -> Result<(), HooksCliError> {
    bootstrap_cli_logging(common.logging_configuration());
    let output = resolve_config_output(&common.output)?;
    let config = load_config_from_store(store, output)?;
    let results = config_provider_statuses(&config);

    match output.format {
        HooksOutputFormat::Text => {
            let text = render_config_providers_text(&results);
            if !text.is_empty() {
                let _ = writeln!(stdout, "{text}");
            }
        }
        HooksOutputFormat::Json => {
            let _ = print_json(&results, output.pretty, stdout);
        }
    }
    Ok(())
}

/// Runs `ariadusage config enable` or `ariadusage config disable` against `store`.
pub fn run_config_set_provider_enabled_with_store(
    args: &ConfigProviderToggleArgs,
    enabled: bool,
    store: &ConfigStore,
    stdout: &mut dyn Write,
) -> Result<(), HooksCliError> {
    bootstrap_cli_logging(args.common.logging_configuration());
    let output = resolve_config_output(&args.common.output)?;
    let Some(descriptor) = resolve_config_provider(args.provider.as_deref()) else {
        return Err(args_failure(
            "Unknown or missing provider. Use --provider <name>.",
            output,
        ));
    };

    let config = load_config_from_store(store, output)?;
    let updated = config_setting_provider_enabled(&config, &descriptor.id, enabled);
    save_config_to_store(store, &updated, output)?;

    let result = ConfigProviderToggleResult {
        provider: descriptor.id.as_str().to_string(),
        display_name: descriptor.display_name.to_string(),
        enabled,
        config_path: store.path().display().to_string(),
    };

    match output.format {
        HooksOutputFormat::Text => {
            let _ = writeln!(stdout, "{}", render_config_provider_toggle_text(&result));
        }
        HooksOutputFormat::Json => {
            let _ = print_json(&result, output.pretty, stdout);
        }
    }
    Ok(())
}

/// Runs `ariadusage config set-source` against `store`.
pub fn run_config_set_source_with_store(
    args: &ConfigSetSourceArgs,
    store: &ConfigStore,
    stdout: &mut dyn Write,
) -> Result<(), HooksCliError> {
    bootstrap_cli_logging(args.common.logging_configuration());
    let output = resolve_config_output(&args.common.output)?;
    let (descriptor, source) =
        config_source_selection(args.provider.as_deref(), args.source.as_deref())
            .map_err(|msg| args_failure(msg, output))?;

    let config = load_config_from_store(store, output)?;
    let updated = config_setting_source(&config, &descriptor.id, source);
    save_config_to_store(store, &updated, output)?;

    let enabled = updated
        .provider_config(descriptor.id.as_str())
        .and_then(|c| c.enabled)
        .unwrap_or(descriptor.default_enabled);

    let result = ConfigSetSourceResult {
        provider: descriptor.id.as_str().to_string(),
        display_name: descriptor.display_name.to_string(),
        enabled,
        source: source_mode_str(source).to_string(),
        config_path: store.path().display().to_string(),
    };

    match output.format {
        HooksOutputFormat::Text => {
            let _ = writeln!(stdout, "{}", render_config_set_source_text(&result));
        }
        HooksOutputFormat::Json => {
            let _ = print_json(&result, output.pretty, stdout);
        }
    }
    Ok(())
}
