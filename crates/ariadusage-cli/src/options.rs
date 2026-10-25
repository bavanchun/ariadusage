// Ported from CodexBar Sources/CodexBarCLI/CLIOptions.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBarCLI/CLIUsageFetchSetup.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBarCLI/CLIHelpers.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBarCLI/CLIErrorReporting.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBarCLI/CLIEntry.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBarCLI/CLIUsageCommand.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBarCLI/TokenAccountCLI.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBarCLI/CLIIO.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

use std::collections::HashMap;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use ariadusage_protocol::{CliErrorKind, ProviderErrorPayload, ProviderPayload};

use crate::exit_code::ExitCode;
use crate::format::{ResetTimeDisplayStyle, format_reset_line};
use crate::output_preferences::{
    CLIOutputPreferences, OutputFormat, ParsedValues, resolve_usage_output_preferences,
};
use crate::provider_meta::{CliProvider, ProviderSelection};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CliSourceMode {
    Auto,
    Web,
    Cli,
    Oauth,
    Api,
}

impl CliSourceMode {
    #[must_use]
    pub const fn raw_value(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Web => "web",
            Self::Cli => "cli",
            Self::Oauth => "oauth",
            Self::Api => "api",
        }
    }

    #[must_use]
    pub fn from_raw(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "auto" => Some(Self::Auto),
            "web" => Some(Self::Web),
            "cli" => Some(Self::Cli),
            "oauth" => Some(Self::Oauth),
            "api" => Some(Self::Api),
            _ => None,
        }
    }

    #[must_use]
    pub const fn requires_web(self) -> bool {
        matches!(self, Self::Web)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TokenAccountCliSelection {
    pub label: Option<String>,
    /// 0-based account index (`--account-index 1` maps to `Some(0)`).
    pub index: Option<isize>,
    pub all_accounts: bool,
}

impl TokenAccountCliSelection {
    #[must_use]
    pub fn new(label: Option<String>, index: Option<isize>, all_accounts: bool) -> Self {
        let label = label
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        Self {
            label,
            index,
            all_accounts,
        }
    }

    #[must_use]
    pub fn has_selection(&self) -> bool {
        self.label.is_some() || self.index.is_some() || self.all_accounts
    }

    #[must_use]
    pub fn uses_override(&self) -> bool {
        self.has_selection()
    }

    #[must_use]
    pub fn provider_selection_error(&self, providers: &[CliProvider]) -> Option<String> {
        if !self.uses_override() {
            return None;
        }
        if providers.len() != 1 {
            return Some("account selection requires a single provider.".to_string());
        }
        None
    }
}

pub fn resolve_provider_token_accounts(
    provider: CliProvider,
    configured_source: CliSourceMode,
    source_mode_override: Option<CliSourceMode>,
    account_set: Option<&TokenAccountSet>,
    selection: &TokenAccountCliSelection,
) -> Result<Vec<TokenAccountEntry>, String> {
    let effective_source = source_mode_override.unwrap_or(configured_source);
    if provider == CliProvider::Antigravity && effective_source == CliSourceMode::Cli {
        if selection.uses_override() {
            return Err("Antigravity CLI uses its local login and cannot select saved Google accounts. Use --source auto or --source oauth with account selection.".to_string());
        }
        return Ok(Vec::new());
    }
    resolve_token_accounts(provider, true, account_set, selection).map_err(|e| e.to_string())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TokenAccountEntry {
    pub id: String,
    pub label: String,
    pub token: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TokenAccountSet {
    pub active_index: usize,
    pub accounts: Vec<TokenAccountEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TokenAccountSelectionError {
    ConflictingFlags,
    BothLabelAndIndex,
    AllAccountsWithExplicitAccount,
    ProviderDoesNotSupportAccounts(CliProvider),
    NoAccountsConfigured(CliProvider),
    AccountNotFound {
        provider: CliProvider,
        label: String,
    },
    AccountIndexOutOfRange {
        provider: CliProvider,
        /// 1-based index displayed in error message.
        index: isize,
        count: usize,
    },
}

impl std::fmt::Display for TokenAccountSelectionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ConflictingFlags
            | Self::BothLabelAndIndex
            | Self::AllAccountsWithExplicitAccount => {
                f.write_str("Use either --account/--account-index or --all-accounts, not both.")
            }
            Self::ProviderDoesNotSupportAccounts(provider) => {
                write!(
                    f,
                    "Provider '{}' does not support token accounts.",
                    provider.raw_value()
                )
            }
            Self::NoAccountsConfigured(provider) => {
                write!(
                    f,
                    "No token accounts configured for {}.",
                    provider.raw_value()
                )
            }
            Self::AccountNotFound { provider, label } => {
                write!(
                    f,
                    "No token account labeled '{label}' for {}.",
                    provider.raw_value()
                )
            }
            Self::AccountIndexOutOfRange {
                provider,
                index,
                count,
            } => {
                write!(
                    f,
                    "Account index {index} out of range for {} (1-{count}).",
                    provider.raw_value()
                )
            }
        }
    }
}

impl std::error::Error for TokenAccountSelectionError {}

pub fn resolve_token_accounts(
    provider: CliProvider,
    supports_token_accounts: bool,
    account_set: Option<&TokenAccountSet>,
    selection: &TokenAccountCliSelection,
) -> Result<Vec<TokenAccountEntry>, TokenAccountSelectionError> {
    if selection.all_accounts && (selection.label.is_some() || selection.index.is_some()) {
        return Err(TokenAccountSelectionError::ConflictingFlags);
    }
    if selection.label.is_some() && selection.index.is_some() {
        return Err(TokenAccountSelectionError::ConflictingFlags);
    }
    if !supports_token_accounts {
        if selection.has_selection() {
            return Err(TokenAccountSelectionError::ProviderDoesNotSupportAccounts(
                provider,
            ));
        }
        return Ok(Vec::new());
    }
    let Some(set) = account_set.filter(|s| !s.accounts.is_empty()) else {
        if selection.has_selection() {
            return Err(TokenAccountSelectionError::NoAccountsConfigured(provider));
        }
        return Ok(Vec::new());
    };

    if selection.all_accounts {
        return Ok(set.accounts.clone());
    }
    if let Some(label) = selection.label.as_deref() {
        let needle = label.trim().to_ascii_lowercase();
        if let Some(found) = set
            .accounts
            .iter()
            .find(|a| a.label.trim().to_ascii_lowercase() == needle)
        {
            return Ok(vec![found.clone()]);
        }
        return Err(TokenAccountSelectionError::AccountNotFound {
            provider,
            label: label.to_string(),
        });
    }
    if let Some(index) = selection.index {
        if index < 0 || (index as usize) >= set.accounts.len() {
            return Err(TokenAccountSelectionError::AccountIndexOutOfRange {
                provider,
                index: index + 1,
                count: set.accounts.len(),
            });
        }
        return Ok(vec![set.accounts[index as usize].clone()]);
    }
    let clamped = set.active_index.min(set.accounts.len() - 1);
    Ok(vec![set.accounts[clamped].clone()])
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsageFetchSetup {
    pub selection: ProviderSelection,
    pub source_mode: CliSourceMode,
    pub fallback_account_email: Option<String>,
}

impl UsageFetchSetup {
    #[must_use]
    pub fn resolve<F>(
        explicit_provider: Option<&str>,
        enabled_providers: &[CliProvider],
        source_mode: CliSourceMode,
        load_fallback_account_email: F,
    ) -> Option<Self>
    where
        F: FnOnce() -> Option<String>,
    {
        let selection = ProviderSelection::from_raw(explicit_provider, enabled_providers)?;
        let includes_codex = selection
            .as_list(CliProvider::ALL)
            .contains(&CliProvider::Codex);
        let fallback_account_email = if includes_codex {
            load_fallback_account_email()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
        } else {
            None
        };
        Some(Self {
            selection,
            source_mode,
            fallback_account_email,
        })
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct UsageOptions {
    pub provider: Option<String>,
    pub account: Option<String>,
    pub account_index: Option<usize>,
    pub all_accounts: bool,
    pub format: Option<String>,
    pub json_shortcut: bool,
    pub json_only: bool,
    pub json_output: bool,
    pub source: Option<String>,
    pub web: bool,
    pub web_timeout: Option<f64>,
    pub web_debug_dump_html: bool,
    pub antigravity_plan_debug: bool,
    pub status: bool,
    pub pretty: bool,
    pub no_color: bool,
    pub no_credits: bool,
    pub brief: bool,
    pub verbose: bool,
    pub log_level: Option<String>,
    pub app_auto_verifier: bool,
    pub help: bool,
    pub version: bool,
}

impl UsageOptions {
    /// Returns `true` when any flag that modifies how provider usage is fetched was supplied,
    /// requiring an in-process fetch rather than serving the daemon's cached snapshot.
    #[must_use]
    pub const fn has_fetch_affecting_flags(&self) -> bool {
        self.source.is_some()
            || self.web
            || self.web_timeout.is_some()
            || self.web_debug_dump_html
            || self.antigravity_plan_debug
            || self.account.is_some()
            || self.account_index.is_some()
            || self.all_accounts
            || self.app_auto_verifier
    }

    #[must_use]
    pub fn to_parsed_values(&self) -> ParsedValues {
        let mut parsed = ParsedValues::default();
        if let Some(p) = &self.provider {
            parsed
                .options
                .insert("provider".to_string(), vec![p.clone()]);
        }
        if let Some(a) = &self.account {
            parsed
                .options
                .insert("account".to_string(), vec![a.clone()]);
        }
        if let Some(idx) = self.account_index {
            parsed
                .options
                .insert("accountIndex".to_string(), vec![idx.to_string()]);
        }
        if self.all_accounts {
            parsed.flags.insert("allAccounts".to_string());
        }
        if let Some(fmt) = &self.format {
            parsed
                .options
                .insert("format".to_string(), vec![fmt.clone()]);
        }
        if self.json_shortcut {
            parsed.flags.insert("jsonShortcut".to_string());
            parsed.flags.insert("json".to_string());
        }
        if self.json_only {
            parsed.flags.insert("jsonOnly".to_string());
        }
        if self.json_output {
            parsed.flags.insert("jsonOutput".to_string());
        }
        if let Some(src) = &self.source {
            parsed
                .options
                .insert("source".to_string(), vec![src.clone()]);
        }
        if self.web {
            parsed.flags.insert("web".to_string());
        }
        if let Some(wt) = self.web_timeout {
            parsed
                .options
                .insert("webTimeout".to_string(), vec![wt.to_string()]);
        }
        if self.web_debug_dump_html {
            parsed.flags.insert("webDebugDumpHtml".to_string());
        }
        if self.antigravity_plan_debug {
            parsed.flags.insert("antigravityPlanDebug".to_string());
        }
        if self.status {
            parsed.flags.insert("status".to_string());
        }
        if self.pretty {
            parsed.flags.insert("pretty".to_string());
        }
        if self.no_color {
            parsed.flags.insert("noColor".to_string());
        }
        if self.no_credits {
            parsed.flags.insert("noCredits".to_string());
        }
        if self.brief {
            parsed.flags.insert("brief".to_string());
        }
        if self.verbose {
            parsed.flags.insert("verbose".to_string());
        }
        if let Some(ll) = &self.log_level {
            parsed
                .options
                .insert("logLevel".to_string(), vec![ll.clone()]);
        }
        if self.app_auto_verifier {
            parsed.flags.insert("appAutoVerifier".to_string());
        }
        parsed
    }
}

pub fn parse_usage_args(args: &[String]) -> Result<(UsageOptions, ParsedValues), String> {
    parse_usage_or_cards_args(args, false)
}

pub fn parse_cards_args(args: &[String]) -> Result<(UsageOptions, ParsedValues), String> {
    parse_usage_or_cards_args(args, true)
}

fn parse_usage_or_cards_args(
    args: &[String],
    cards_command: bool,
) -> Result<(UsageOptions, ParsedValues), String> {
    let mut opts = UsageOptions::default();
    let mut parsed = ParsedValues::default();
    let mut idx = 0;

    let push_opt = |parsed: &mut ParsedValues, key: &str, val: String| {
        parsed.options.entry(key.to_string()).or_default().push(val);
    };

    while idx < args.len() {
        let token = &args[idx];
        let (flag, inline_val) = if let Some((k, v)) = token.split_once('=') {
            if k.starts_with('-') {
                (k, Some(v.to_string()))
            } else {
                (token.as_str(), None)
            }
        } else {
            (token.as_str(), None)
        };

        let mut take_val = |flag_name: &str| -> Result<String, String> {
            if let Some(v) = inline_val.clone() {
                Ok(v)
            } else if idx + 1 < args.len() {
                idx += 1;
                Ok(args[idx].clone())
            } else {
                Err(format!("Missing value for option {flag_name}"))
            }
        };

        match flag {
            "-h" | "--help" => {
                opts.help = true;
                parsed.flags.insert("help".to_string());
            }
            "-V" | "--version" => {
                opts.version = true;
                parsed.flags.insert("version".to_string());
            }
            "-v" | "--verbose" => {
                opts.verbose = true;
                parsed.flags.insert("verbose".to_string());
            }
            "--json" => {
                opts.json_shortcut = true;
                parsed.flags.insert("jsonShortcut".to_string());
            }
            "--json-output" => {
                opts.json_output = true;
                parsed.flags.insert("jsonOutput".to_string());
            }
            "--json-only" => {
                opts.json_only = true;
                parsed.flags.insert("jsonOnly".to_string());
            }
            "--app-auto-verifier" if !cards_command => {
                opts.app_auto_verifier = true;
                parsed.flags.insert("appAutoVerifier".to_string());
            }
            "--pretty" => {
                opts.pretty = true;
                parsed.flags.insert("pretty".to_string());
            }
            "--no-color" => {
                opts.no_color = true;
                parsed.flags.insert("noColor".to_string());
            }
            "--no-credits" => {
                opts.no_credits = true;
                parsed.flags.insert("noCredits".to_string());
            }
            "--status" => {
                opts.status = true;
                parsed.flags.insert("status".to_string());
            }
            "--web" => {
                opts.web = true;
                parsed.flags.insert("web".to_string());
            }
            "--web-debug-dump-html" => {
                opts.web_debug_dump_html = true;
                parsed.flags.insert("webDebugDumpHtml".to_string());
            }
            "--antigravity-plan-debug" => {
                opts.antigravity_plan_debug = true;
                parsed.flags.insert("antigravityPlanDebug".to_string());
            }
            "--all-accounts" => {
                opts.all_accounts = true;
                parsed.flags.insert("allAccounts".to_string());
            }
            "--brief" if cards_command => {
                opts.brief = true;
                parsed.flags.insert("brief".to_string());
            }
            "--provider" => {
                let val = take_val("--provider")?;
                opts.provider = Some(val.clone());
                push_opt(&mut parsed, "provider", val);
            }
            "--account" => {
                let val = take_val("--account")?;
                opts.account = Some(val.clone());
                push_opt(&mut parsed, "account", val);
            }
            "--account-index" => {
                let val = take_val("--account-index")?;
                if let Ok(parsed_idx) = val.parse::<i64>()
                    && parsed_idx > 0
                {
                    opts.account_index = Some(parsed_idx as usize);
                }
                push_opt(&mut parsed, "accountIndex", val);
            }
            "--format" => {
                let val = take_val(flag)?;
                opts.format = Some(val.clone());
                push_opt(&mut parsed, "format", val);
            }
            "--source" => {
                let val = take_val("--source")?;
                opts.source = Some(val.clone());
                push_opt(&mut parsed, "source", val);
            }
            "--web-timeout" => {
                let val = take_val("--web-timeout")?;
                push_opt(&mut parsed, "webTimeout", val);
                if let Ok(secs) = decode_web_timeout(&parsed) {
                    opts.web_timeout = secs;
                }
            }
            "--log-level" => {
                let val = take_val("--log-level")?;
                opts.log_level = Some(val.clone());
                push_opt(&mut parsed, "logLevel", val);
            }
            other if other.starts_with('-') => {
                return Err(format!("Unknown option {flag}"));
            }
            other => {
                parsed.positional.push(other.to_string());
            }
        }
        idx += 1;
    }

    Ok((opts, parsed))
}

#[must_use]
pub fn simulate_cli_parse_failure(raw_argv: &[String]) -> (ExitCode, String, String) {
    let argv = effective_argv(raw_argv);
    let output_preferences = CLIOutputPreferences::from_argv(&argv);
    let cmd = argv.first().map_or("usage", String::as_str);
    let rest = if argv.is_empty() { &[][..] } else { &argv[1..] };
    let res = if cmd == "cards" {
        parse_cards_args(rest)
    } else {
        parse_usage_args(rest)
    };
    match res {
        Ok(_) => (ExitCode::Success, String::new(), String::new()),
        Err(message) => {
            if output_preferences.uses_json_output() {
                let payload = make_cli_error_provider_payload(
                    &message,
                    ExitCode::Failure,
                    CliErrorKind::Args,
                );
                let out = render_provider_payloads(&[payload], output_preferences)
                    .map_or_else(String::new, |s| format!("{s}\n"));
                (ExitCode::Failure, out, String::new())
            } else {
                (
                    ExitCode::Failure,
                    String::new(),
                    format!("Error: {message}\n"),
                )
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CliArgumentValidationError {
    InvalidAccountIndex,
    InvalidWebTimeout,
}

impl std::fmt::Display for CliArgumentValidationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidAccountIndex => f.write_str("--account-index must be a positive integer."),
            Self::InvalidWebTimeout => f.write_str(
                "--web-timeout must be a finite, nonnegative number within the supported range.",
            ),
        }
    }
}

impl std::error::Error for CliArgumentValidationError {}

#[must_use]
pub fn decode_format(values: &ParsedValues) -> OutputFormat {
    CLIOutputPreferences::resolve_output_format_from_values(values, false).format
}

pub fn decode_token_account_selection(
    values: &ParsedValues,
) -> Result<TokenAccountCliSelection, CliArgumentValidationError> {
    let label = values
        .options
        .get("account")
        .and_then(|v| v.last())
        .cloned();
    let index = match values.options.get("accountIndex").and_then(|v| v.last()) {
        Some(raw) => {
            let Ok(value) = raw.parse::<i64>() else {
                return Err(CliArgumentValidationError::InvalidAccountIndex);
            };
            if value <= 0 {
                return Err(CliArgumentValidationError::InvalidAccountIndex);
            }
            Some((value - 1) as isize)
        }
        None => None,
    };
    let all_accounts = values.flags.contains("allAccounts");
    Ok(TokenAccountCliSelection::new(label, index, all_accounts))
}

#[must_use]
pub fn app_auto_verifier_argument_error(
    enabled: bool,
    providers: &[CliProvider],
    source_mode: Option<CliSourceMode>,
    token_selection: &TokenAccountCliSelection,
) -> Option<String> {
    if !enabled {
        return None;
    }
    if providers != [CliProvider::Claude] || source_mode != Some(CliSourceMode::Auto) {
        return Some("--app-auto-verifier requires --provider claude --source auto.".to_string());
    }
    if token_selection.uses_override() {
        return Some("--app-auto-verifier does not accept token-account selection.".to_string());
    }
    None
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ProviderRuntime {
    #[default]
    Cli,
    App,
}

#[derive(Debug, Clone, PartialEq)]
pub struct UsageCommandContext {
    pub format: OutputFormat,
    pub output: CLIOutputPreferences,
    pub include_status: bool,
    pub include_credits: bool,
    pub source_mode_override: Option<CliSourceMode>,
    pub reset_style: ResetTimeDisplayStyle,
    pub no_credits: bool,
    pub pretty: bool,
    pub json_only: bool,
    pub verbose: bool,
    pub antigravity_plan_debug: bool,
    pub web_debug_dump_html: bool,
    pub web_timeout: f64,
    pub source_mode: CliSourceMode,
    pub use_color: bool,
    pub include_all_codex_accounts: bool,
    pub provider_runtime: ProviderRuntime,
    pub cards_layout: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CLIUsageFetchSetup {
    pub token_selection: TokenAccountCliSelection,
    pub include_status: bool,
    pub command: UsageCommandContext,
}

impl CLIUsageFetchSetup {
    pub fn new(
        values: &ParsedValues,
        providers: &[CliProvider],
        output: CLIOutputPreferences,
        cards_layout: bool,
    ) -> Result<Self, String> {
        let source_mode_override = decode_source_mode(values);
        if values.options.get("source").is_some_and(|v| !v.is_empty())
            && source_mode_override.is_none()
        {
            return Err("--source must be auto|web|cli|oauth|api.".to_string());
        }
        let web_timeout = decode_web_timeout(values)?.unwrap_or(60.0);
        let selection = decode_token_account_selection(values).map_err(|e| e.to_string())?;
        if selection.all_accounts && (selection.label.is_some() || selection.index.is_some()) {
            return Err(
                "--all-accounts cannot be combined with --account or --account-index.".to_string(),
            );
        }
        let app_auto_verifier = !cards_layout && values.flags.contains("appAutoVerifier");
        if let Some(msg) = app_auto_verifier_argument_error(
            app_auto_verifier,
            providers,
            source_mode_override,
            &selection,
        ) {
            return Err(msg);
        }
        if let Some(msg) = selection.provider_selection_error(providers) {
            return Err(msg);
        }
        let include_status = values.flags.contains("status");
        let format = if cards_layout {
            OutputFormat::Text
        } else {
            output.format
        };
        let no_credits = values.flags.contains("noCredits");
        let include_credits = matches!(format, OutputFormat::Json) || !no_credits;
        let use_color =
            CLIOutputPreferences::should_use_color_stdout(values.flags.contains("noColor"), format);
        let command = UsageCommandContext {
            format,
            output,
            include_status,
            include_credits,
            source_mode_override,
            reset_style: ResetTimeDisplayStyle::Countdown,
            no_credits,
            pretty: output.pretty,
            json_only: output.json_only,
            verbose: values.flags.contains("verbose"),
            antigravity_plan_debug: values.flags.contains("antigravityPlanDebug"),
            web_debug_dump_html: values.flags.contains("webDebugDumpHtml"),
            web_timeout,
            source_mode: source_mode_override.unwrap_or(CliSourceMode::Auto),
            use_color,
            include_all_codex_accounts: selection.all_accounts && providers == [CliProvider::Codex],
            provider_runtime: if app_auto_verifier {
                ProviderRuntime::App
            } else {
                ProviderRuntime::Cli
            },
            cards_layout,
        };
        Ok(Self {
            token_selection: selection,
            include_status,
            command,
        })
    }

    pub fn parse_context(
        values: &ParsedValues,
        provider: &ProviderSelection,
        is_tty: bool,
        env: &HashMap<String, String>,
        reset_time_display_style_is_absolute: bool,
        is_macos: bool,
    ) -> Result<UsageCommandContext, (ExitCode, String)> {
        let output = resolve_usage_output_preferences(values);
        let format = output.format;
        let include_status = values.flags.contains("status");
        let no_color = values.flags.contains("noColor");
        let no_credits = values.flags.contains("noCredits");
        let pretty = output.pretty;
        let json_only = output.json_only;
        let verbose = values.flags.contains("verbose");
        let antigravity_plan_debug = values.flags.contains("antigravityPlanDebug");
        let web_debug_dump_html = values.flags.contains("webDebugDumpHtml");
        let web_timeout = decode_web_timeout(values)
            .map_err(|msg| (ExitCode::Failure, format!("Error: {msg}")))?
            .unwrap_or(60.0);
        let source_mode = validate_source_mode(values, provider, is_macos)?;
        let term_is_dumb = env
            .get("TERM")
            .is_some_and(|t| t.eq_ignore_ascii_case("dumb"));
        let use_color =
            CLIOutputPreferences::should_use_color(no_color, format, is_tty && !term_is_dumb);
        let reset_style = if reset_time_display_style_is_absolute {
            ResetTimeDisplayStyle::Absolute
        } else {
            ResetTimeDisplayStyle::Countdown
        };

        Ok(UsageCommandContext {
            format,
            output,
            include_status,
            include_credits: matches!(format, OutputFormat::Json) || !no_credits,
            source_mode_override: decode_source_mode(values),
            reset_style,
            no_credits,
            pretty,
            json_only,
            verbose,
            antigravity_plan_debug,
            web_debug_dump_html,
            web_timeout,
            source_mode,
            use_color,
            include_all_codex_accounts: false,
            provider_runtime: ProviderRuntime::Cli,
            cards_layout: false,
        })
    }
}

pub use crate::logging::{CliLogLevel, resolved_log_level};

pub fn decode_web_timeout(values: &ParsedValues) -> Result<Option<f64>, String> {
    let Some(raw) = values.options.get("webTimeout").and_then(|v| v.last()) else {
        return Ok(None);
    };
    let Ok(seconds) = raw.parse::<f64>() else {
        return Err(
            "--web-timeout must be a finite, nonnegative number within the supported range."
                .to_string(),
        );
    };
    if !seconds.is_finite() || seconds < 0.0 || seconds > (i64::MAX as f64) {
        return Err(
            "--web-timeout must be a finite, nonnegative number within the supported range."
                .to_string(),
        );
    }
    Ok(Some(seconds))
}

#[must_use]
pub fn decode_source_mode(values: &ParsedValues) -> Option<CliSourceMode> {
    if values.flags.contains("web") {
        return Some(CliSourceMode::Web);
    }
    let raw = values.options.get("source")?.last()?;
    CliSourceMode::from_raw(raw)
}

pub fn validate_source_mode(
    values: &ParsedValues,
    selection: &ProviderSelection,
    is_macos: bool,
) -> Result<CliSourceMode, (ExitCode, String)> {
    if values.flags.contains("web") {
        let source_mode = CliSourceMode::Web;
        check_web_platform(source_mode, selection, is_macos)?;
        return Ok(source_mode);
    }
    if let Some(raw) = values.options.get("source").and_then(|v| v.last()) {
        let Some(source_mode) = CliSourceMode::from_raw(raw) else {
            return Err((
                ExitCode::Failure,
                "Error: --source must be auto|web|cli|oauth|api".to_string(),
            ));
        };
        check_web_platform(source_mode, selection, is_macos)?;
        return Ok(source_mode);
    }
    Ok(CliSourceMode::Auto)
}

fn check_web_platform(
    source_mode: CliSourceMode,
    _selection: &ProviderSelection,
    is_macos: bool,
) -> Result<(), (ExitCode, String)> {
    if !is_macos && source_mode.requires_web() {
        return Err((
            ExitCode::Failure,
            "Error: --source web/--web is only supported on macOS.".to_string(),
        ));
    }
    Ok(())
}

#[must_use]
pub fn effective_argv(argv: &[String]) -> Vec<String> {
    let Some(first) = argv.first() else {
        return vec!["usage".to_string()];
    };
    if first.starts_with('-') {
        let mut out = Vec::with_capacity(argv.len() + 1);
        out.push("usage".to_string());
        out.extend_from_slice(argv);
        return out;
    }
    match first.as_str() {
        "usage" | "cards" | "status" | "cost" | "cache" | "config" | "hooks" | "diagnose"
        | "guard" | "serve" | "daemon" | "secret" | "cookie" | "sessions" => argv.to_vec(),
        _ => {
            let mut out = Vec::with_capacity(argv.len() + 1);
            out.push("usage".to_string());
            out.extend_from_slice(argv);
            out
        }
    }
}

#[must_use]
pub fn format_header(provider: CliProvider, version: Option<&str>, source: &str) -> String {
    let name = provider.display_name();
    if let Some(version) = version.filter(|s| !s.is_empty()) {
        format!("{name} {version} ({source})")
    } else {
        format!("{name} ({source})")
    }
}

#[must_use]
pub fn make_error_payload(
    code: ExitCode,
    message: impl Into<String>,
    kind: CliErrorKind,
) -> ProviderErrorPayload {
    ProviderErrorPayload::new(code.raw_value(), message, Some(kind))
}

#[must_use]
pub fn make_cli_error_provider_payload(
    message: &str,
    code: ExitCode,
    kind: CliErrorKind,
) -> ProviderPayload {
    ProviderPayload {
        provider: "cli".to_string(),
        account: None,
        version: None,
        source: "cli".to_string(),
        status: None,
        usage: None,
        rate_window_labels: None,
        credits: None,
        reset_credits: None,
        antigravity_plan_info: None,
        openai_dashboard: None,
        diagnostic: None,
        error: Some(make_error_payload(code, message, kind)),
        pace: None,
        age_seconds: None,
    }
}

#[must_use]
pub fn encode_json<T: serde::Serialize + ?Sized>(value: &T, pretty: bool) -> Option<String> {
    crate::toon::ToonFormatter::to_node(value).ok()?;
    if pretty {
        serde_json::to_string_pretty(value).ok()
    } else {
        serde_json::to_string(value).ok()
    }
}

#[must_use]
pub fn render_provider_payloads(
    payloads: &[ProviderPayload],
    output: CLIOutputPreferences,
) -> Option<String> {
    if output.toon_requested {
        crate::toon::ToonFormatter::encode(payloads)
            .ok()
            .filter(|s| !s.is_empty())
    } else {
        encode_json(payloads, output.pretty)
    }
}

#[must_use]
pub fn make_cli_error_payload(
    message: &str,
    code: ExitCode,
    kind: CliErrorKind,
    pretty: bool,
) -> Option<String> {
    let payload = make_cli_error_provider_payload(message, code, kind);
    encode_json(&vec![payload], pretty)
}

#[must_use]
pub const fn should_print_exit_error(code: ExitCode, message: Option<&str>) -> bool {
    !matches!(code, ExitCode::Success) && message.is_some()
}

#[must_use]
pub fn should_detect_version(provider: CliProvider, strategy_kind: FetchAttemptKind) -> bool {
    !(provider == CliProvider::Claude && strategy_kind == FetchAttemptKind::Oauth)
}

#[must_use]
pub fn holds_antigravity_cli_session_for_plan_debug(
    provider: CliProvider,
    plan_debug_enabled: bool,
    json_only: bool,
    persists_cli_sessions: bool,
) -> bool {
    provider == CliProvider::Antigravity
        && plan_debug_enabled
        && !json_only
        && !persists_cli_sessions
}

#[must_use]
pub fn usage_text_notes(
    _provider: CliProvider,
    _source_mode: CliSourceMode,
    _resolved_source_label: &str,
) -> Vec<String> {
    Vec::new()
}

#[must_use]
pub fn normalize_version(raw: Option<&str>) -> Option<String> {
    let trimmed = raw?.trim();
    if trimmed.is_empty() {
        return None;
    }
    let bytes = trimmed.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i].is_ascii_digit() {
            let start = i;
            let mut j = i + 1;
            let mut has_dot = false;
            let mut last_was_digit = true;
            while j < bytes.len() {
                if bytes[j].is_ascii_digit() {
                    last_was_digit = true;
                    j += 1;
                } else if bytes[j] == b'.'
                    && last_was_digit
                    && j + 1 < bytes.len()
                    && bytes[j + 1].is_ascii_digit()
                {
                    has_dot = true;
                    last_was_digit = false;
                    j += 1;
                } else {
                    break;
                }
            }
            if has_dot && last_was_digit {
                return Some(trimmed[start..j].to_string());
            }
            i = j;
        } else {
            i += 1;
        }
    }
    Some(trimmed.to_string())
}

pub fn next_ancestor<F>(url: &Path, parent_provider: F) -> Option<PathBuf>
where
    F: FnOnce(&Path) -> PathBuf,
{
    let parent = parent_provider(url);
    if parent.as_os_str().is_empty() || parent == url || !url.starts_with(&parent) {
        None
    } else if parent.components().count() < url.components().count() {
        Some(parent)
    } else {
        None
    }
}

#[must_use]
pub fn containing_app_version(executable_path: &Path) -> Option<String> {
    let resolved =
        std::fs::canonicalize(executable_path).unwrap_or_else(|_| executable_path.to_path_buf());
    let mut cursor = resolved;
    while let Some(parent) = next_ancestor(&cursor, |p| {
        p.parent().unwrap_or_else(|| Path::new("")).to_path_buf()
    }) {
        cursor = parent;
        if cursor.extension().and_then(|e| e.to_str()) == Some("app") {
            let info_plist = cursor.join("Contents").join("Info.plist");
            if let Ok(contents) = std::fs::read_to_string(&info_plist)
                && let Some(ver) = extract_plist_string(&contents, "CFBundleShortVersionString")
                && let Some(normalized) = normalize_version(Some(&ver))
            {
                return Some(normalized);
            }
            return None;
        }
    }
    None
}

fn extract_plist_string(xml: &str, key: &str) -> Option<String> {
    let needle = format!("<key>{key}</key>");
    let idx = xml.find(&needle)?;
    let rest = &xml[idx + needle.len()..];
    let s_start = rest.find("<string>")?;
    let after_start = &rest[s_start + "<string>".len()..];
    let s_end = after_start.find("</string>")?;
    Some(after_start[..s_end].to_string())
}

#[must_use]
pub fn adjacent_version_file_version(executable_path: &Path) -> Option<String> {
    let resolved =
        std::fs::canonicalize(executable_path).unwrap_or_else(|_| executable_path.to_path_buf());
    let dir = resolved.parent()?;
    for name in ["VERSION", ".ariadusage-version"] {
        let candidate = dir.join(name);
        if let Ok(raw) = std::fs::read_to_string(&candidate) {
            let trimmed = raw.trim();
            if trimmed.is_empty() {
                continue;
            }
            if let Some(rest) = trimmed
                .strip_prefix('v')
                .or_else(|| trimmed.strip_prefix('V'))
                && rest.chars().next().is_some_and(|c| c.is_ascii_digit())
            {
                return Some(rest.to_string());
            }
            return Some(trimmed.to_string());
        }
    }
    None
}

#[must_use]
pub fn current_version_from(
    bundle_version: Option<&str>,
    executable_path: Option<&Path>,
    fallback_version: &str,
) -> String {
    if let Some(bv) = bundle_version
        && bv.chars().any(|c| c.is_ascii_digit())
        && let Some(version) = normalize_version(Some(bv))
    {
        return version;
    }
    if let Some(exe) = executable_path {
        if let Some(version) = containing_app_version(exe) {
            return version;
        }
        if let Some(version) = adjacent_version_file_version(exe) {
            return version;
        }
    }
    normalize_version(Some(fallback_version)).unwrap_or_else(|| "0.0.0".to_string())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FetchAttemptKind {
    Cli,
    Web,
    Oauth,
    ApiToken,
    LocalProbe,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderFetchAttempt {
    pub strategy_id: String,
    pub kind: FetchAttemptKind,
    pub was_available: bool,
    pub error_description: Option<String>,
}

impl ProviderFetchAttempt {
    #[must_use]
    pub fn new(
        strategy_id: impl Into<String>,
        kind: FetchAttemptKind,
        was_available: bool,
        error_description: Option<String>,
    ) -> Self {
        Self {
            strategy_id: strategy_id.into(),
            kind,
            was_available,
            error_description,
        }
    }
}

#[must_use]
pub fn antigravity_auto_fallback_summary(
    provider: CliProvider,
    source_mode: CliSourceMode,
    attempts: &[ProviderFetchAttempt],
) -> Option<String> {
    if provider != CliProvider::Antigravity
        || source_mode != CliSourceMode::Auto
        || attempts.is_empty()
    {
        return None;
    }
    let parts: Vec<String> = attempts
        .iter()
        .map(|attempt| {
            let source = antigravity_source_short_label(&attempt.strategy_id);
            if !attempt.was_available {
                format!("{source}: skipped (unavailable)")
            } else if let Some(err) = attempt
                .error_description
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
            {
                format!("{source}: {err}")
            } else if attempt.error_description.is_some() {
                format!("{source}: failed")
            } else {
                format!("{source}: success")
            }
        })
        .collect();
    Some(format!(
        "Antigravity auto source outcomes: {}",
        parts.join(" -> ")
    ))
}

fn antigravity_source_short_label(strategy_id: &str) -> String {
    let Some(short) = strategy_id.strip_prefix("antigravity.") else {
        return strategy_id.to_string();
    };
    short.replace("-local", "").replace("-https", "")
}

#[must_use]
pub fn render_openai_web_dashboard_text(
    dash: &ariadusage_protocol::OpenAiDashboardSnapshot,
    now: jiff::Timestamp,
) -> String {
    let mut lines = Vec::new();
    if let Some(email) = dash.signed_in_email.as_deref().filter(|s| !s.is_empty()) {
        lines.push(format!("Web session: {email}"));
    }
    if let Some(remaining) = dash.code_review_remaining_percent {
        let percent = remaining.round() as i64;
        if let Some(limit) = dash.code_review_limit.as_ref()
            && let Some(reset) = format_reset_line(
                limit
                    .resets_at
                    .as_deref()
                    .and_then(|s| s.parse::<jiff::Timestamp>().ok()),
                limit.reset_description.as_deref(),
                ResetTimeDisplayStyle::Countdown,
                now,
            )
        {
            lines.push(format!("Code review: {percent}% remaining ({reset})"));
        } else {
            lines.push(format!("Code review: {percent}% remaining"));
        }
    }
    if let Some(first) = dash.credit_events.first() {
        lines.push(format!(
            "Web history: {} events (latest {})",
            dash.credit_events.len(),
            first.date
        ));
    } else {
        lines.push("Web history: none".to_string());
    }
    lines.join("\n")
}

#[derive(Debug, Clone, Default)]
pub struct CliStderrBuffer {
    inner: Arc<Mutex<String>>,
}

impl CliStderrBuffer {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn append(&self, text: &str) {
        if let Ok(mut guard) = self.inner.lock() {
            guard.push_str(text);
        }
    }

    #[must_use]
    pub fn contents(&self) -> String {
        self.inner
            .lock()
            .map_or_else(|_| String::new(), |g| g.clone())
    }
}

#[derive(Debug, Clone, Default)]
pub struct UsageCommandOutput {
    pub sections: Vec<String>,
    pub cards: Vec<crate::cards_renderer::CLICardModel>,
    pub brief_rows: Vec<crate::cards_brief_renderer::CLICardsBriefRow>,
    pub card_failures: Vec<crate::cards_renderer::CLICardFailure>,
    pub payload: Vec<ProviderPayload>,
    pub exit_code: ExitCode,
}

impl UsageCommandOutput {
    pub fn merge(&mut self, other: Self) {
        self.sections.extend(other.sections);
        self.cards.extend(other.cards);
        self.brief_rows.extend(other.brief_rows);
        self.card_failures.extend(other.card_failures);
        self.payload.extend(other.payload);
        if other.exit_code != ExitCode::Success && self.exit_code == ExitCode::Success {
            self.exit_code = other.exit_code;
        }
    }
}

pub const MAX_CONCURRENT_PROVIDER_FETCHES: usize = 3;

pub async fn collect_usage_outputs<F, Fut>(
    providers: &[CliProvider],
    stderr: Option<&CliStderrBuffer>,
    fetch: F,
) -> UsageCommandOutput
where
    F: Fn(CliProvider, CliStderrBuffer) -> Fut + Send + Sync,
    Fut: Future<Output = UsageCommandOutput> + Send,
{
    if providers.is_empty() {
        return UsageCommandOutput::default();
    }
    if providers.len() == 1 {
        let buf = CliStderrBuffer::new();
        let out = fetch(providers[0], buf.clone()).await;
        if let Some(parent_stderr) = stderr {
            parent_stderr.append(&buf.contents());
        }
        return out;
    }

    let semaphore = Arc::new(tokio::sync::Semaphore::new(
        MAX_CONCURRENT_PROVIDER_FETCHES.min(providers.len()),
    ));
    type ProviderFetchTask<'a> =
        Pin<Box<dyn Future<Output = (usize, UsageCommandOutput, String)> + Send + 'a>>;
    let futs: Vec<ProviderFetchTask<'_>> = providers
        .iter()
        .copied()
        .enumerate()
        .map(|(index, provider)| {
            let sem = Arc::clone(&semaphore);
            let buf = CliStderrBuffer::new();
            let fut = fetch(provider, buf.clone());
            Box::pin(async move {
                let _permit = sem.acquire().await.expect("semaphore open");
                let output = fut.await;
                let diag = buf.contents();
                (index, output, diag)
            }) as ProviderFetchTask<'_>
        })
        .collect();

    let mut results = futures_join_all(futs).await;
    results.sort_by_key(|(idx, _, _)| *idx);

    let mut merged = UsageCommandOutput::default();
    let mut ordered_diags = HashMap::new();
    for (idx, out, diag) in results {
        ordered_diags.insert(idx, diag);
        merged.merge(out);
    }
    if let Some(parent_stderr) = stderr {
        for idx in 0..providers.len() {
            if let Some(diag) = ordered_diags.get(&idx)
                && !diag.is_empty()
            {
                parent_stderr.append(diag);
            }
        }
    }
    merged
}

async fn futures_join_all<T>(
    mut futs: Vec<Pin<Box<dyn Future<Output = T> + Send + '_>>>,
) -> Vec<T> {
    let mut out = Vec::with_capacity(futs.len());
    // Poll all futures concurrently using tokio::select! / Poll
    std::future::poll_fn(|cx| {
        let mut i = 0;
        while i < futs.len() {
            if let std::task::Poll::Ready(val) = futs[i].as_mut().poll(cx) {
                std::mem::drop(futs.swap_remove(i));
                out.push(val);
            } else {
                i += 1;
            }
        }
        if futs.is_empty() {
            std::task::Poll::Ready(std::mem::take(&mut out))
        } else {
            std::task::Poll::Pending
        }
    })
    .await
}
