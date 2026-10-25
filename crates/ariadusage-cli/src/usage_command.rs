// Ported from CodexBar Sources/CodexBarCLI/CLIUsageCommand.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBarCLI/CLICardsCommand.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

use std::path::PathBuf;

use ariadusage_core::config::{Config, ProviderEntry};
use ariadusage_core::pace::UsagePace;
use ariadusage_core::pipeline::{
    DiagnosticFetchAttempt, FetchError, FetchInteraction, FetchRuntime, TransportClass,
    error_kind_to_category,
};
use ariadusage_core::projection::{
    ProjectedError, ProjectionInput, RefreshState, provider_snapshot,
};
use ariadusage_core::providers::{SourceMode, find_by_id};
use ariadusage_engine::config_store::ConfigStore;
use ariadusage_engine::paths;
use ariadusage_protocol::ids::ProviderId;
use ariadusage_protocol::metric::{Confidence, Metric, MetricSource, SourceKind};
use ariadusage_protocol::snapshot::{AccountSnapshot, ProviderWindows};
use ariadusage_protocol::usage::{ProviderError, ProviderErrorCategory, ProviderErrorKind};
use ariadusage_protocol::{
    CliCreditsSnapshot, CliErrorKind, CliUsageSnapshot, EngineSnapshot, ProviderErrorPayload,
    ProviderPayload, ProviderResetCreditsPayload, ProviderSnapshot, ProviderStatusPayload,
};
use jiff::Timestamp;
use jiff::tz::TimeZone;

use crate::cards_brief_renderer::CLICardsBriefRenderer;
use crate::cards_renderer::{CLICardBuildInput, CLICardFailure, CLICardsRenderer};
use crate::claude_swap_cards::{
    CLIClaudeSwapCards, CLIClaudeSwapCardsRenderOptions, ClaudeSwapAccountProjection,
    ClaudeSwapAccountReader, ClaudeSwapAccountSource, ProviderAccountIdentity,
    ProviderAccountUsageSnapshot,
};
use crate::exit_code::ExitCode;
use crate::format::ResetTimeDisplayStyle;
use crate::ipc_client::fetch_daemon_snapshot;
use crate::options::{
    CliSourceMode, FetchAttemptKind, ProviderFetchAttempt, TokenAccountEntry, TokenAccountSet,
    UsageCommandOutput, UsageOptions, antigravity_auto_fallback_summary,
    app_auto_verifier_argument_error, decode_source_mode, decode_token_account_selection,
    decode_web_timeout, format_header, make_cli_error_payload, resolve_provider_token_accounts,
    validate_source_mode,
};
use crate::output_preferences::{
    CLIOutputPreferences, OutputFormat, ParsedValues, resolve_usage_output_preferences,
    should_use_color,
};
use crate::provider_meta::{
    CliProvider, ProviderSelection, make_provider_window_labels_payload, resolved_account_text,
};
use crate::renderer::{CLIRenderer, RenderContext};
use crate::terminal::TerminalCapabilities;
use crate::toon::ToonFormatter;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderedCommandResult {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: i32,
}

/// Resolves the [`crate::logging::CliLoggingConfiguration`] for `usage` or `cards` from
/// `opts` and `parsed`, using [`crate::logging::resolved_log_level`] and routing to `stderr`
/// (with JSON log formatting when `--json-output` or `--json-only` is enabled).
#[must_use]
pub fn usage_logging_configuration(
    opts: &UsageOptions,
    parsed: &ParsedValues,
    force_cards: bool,
) -> crate::logging::CliLoggingConfiguration {
    let verbose = opts.verbose || parsed.flags.contains("verbose");
    let json_output = opts.json_output || parsed.flags.contains("jsonOutput");
    let json_only = opts.json_only || parsed.flags.contains("jsonOnly");
    let log_level = opts.log_level.as_deref().or_else(|| {
        parsed
            .options
            .get("logLevel")
            .and_then(|vals| vals.last())
            .map(String::as_str)
    });
    let command_path = if force_cards {
        &["cards"][..]
    } else {
        &["usage"][..]
    };
    crate::logging::CliLoggingConfiguration::for_command(
        verbose,
        json_output,
        json_only,
        log_level,
        command_path,
    )
}

fn validate_usage_arguments(
    opts: &UsageOptions,
    parsed: &ParsedValues,
    default_enabled: &[CliProvider],
    force_cards: bool,
    output: &CLIOutputPreferences,
) -> Result<(ProviderSelection, Vec<CliProvider>), RenderedCommandResult> {
    if opts.web_debug_dump_html
        || parsed.flags.contains("webDebugDumpHtml")
        || parsed.flags.contains("web-debug-dump-html")
    {
        return Err(render_arg_error(
            "Error: --web-debug-dump-html is not supported; AriadUsage web strategies use JSON APIs and do not parse HTML.",
            output,
        ));
    }
    if opts.account.is_some() && opts.account_index.is_some() {
        return Err(render_arg_error(
            "Error: cannot combine --account and --account-index",
            output,
        ));
    }
    if opts.all_accounts && (opts.account.is_some() || opts.account_index.is_some()) {
        return Err(render_arg_error(
            "Error: cannot combine --all-accounts with --account or --account-index",
            output,
        ));
    }
    if let Err(msg) = decode_web_timeout(parsed) {
        return Err(render_arg_error(&format!("Error: {msg}"), output));
    }
    let token_selection = match decode_token_account_selection(parsed) {
        Ok(sel) => sel,
        Err(msg) => return Err(render_arg_error(&format!("Error: {msg}"), output)),
    };

    let Some(selection) = ProviderSelection::from_raw(opts.provider.as_deref(), default_enabled)
    else {
        return Err(render_arg_error(
            "Error: --provider must be codex|claude|antigravity|both|all",
            output,
        ));
    };

    let selected_providers = selection.as_list(CliProvider::V1_PROVIDERS);
    if let Some(msg) = app_auto_verifier_argument_error(
        !force_cards && opts.app_auto_verifier,
        &selected_providers,
        decode_source_mode(parsed),
        &token_selection,
    ) {
        return Err(render_arg_error(&format!("Error: {msg}"), output));
    }
    if let Some(msg) = token_selection.provider_selection_error(&selected_providers) {
        return Err(render_arg_error(&format!("Error: {msg}"), output));
    }
    if let Err((_code, msg)) = validate_source_mode(parsed, &selection, cfg!(target_os = "macos")) {
        return Err(render_arg_error(&msg, output));
    }

    Ok((selection, selected_providers))
}

pub fn execute_usage_from_snapshot(
    snapshot: &EngineSnapshot,
    opts: &UsageOptions,
    parsed: &ParsedValues,
    force_cards: bool,
    terminal: TerminalCapabilities,
    now: Timestamp,
) -> RenderedCommandResult {
    crate::logging::bootstrap_cli_logging(usage_logging_configuration(opts, parsed, force_cards));
    let output = resolve_usage_output_preferences(parsed);
    let format = output.format;
    let use_color = should_use_color(opts.no_color, format, terminal);
    let enhanced = use_color && terminal.enhanced_cards;
    let use_cards = force_cards && format == OutputFormat::Text && !opts.brief;
    let use_brief = force_cards && format == OutputFormat::Text && opts.brief;
    let reset_style = ResetTimeDisplayStyle::Countdown;

    let enabled_providers: Vec<CliProvider> = snapshot
        .providers
        .iter()
        .filter(|p| p.enabled)
        .filter_map(|p| CliProvider::from_core_provider_id(&p.id))
        .collect();
    let default_enabled = if enabled_providers.is_empty() {
        vec![CliProvider::Codex, CliProvider::Claude]
    } else {
        enabled_providers
    };

    let (_selection, selected_providers) =
        match validate_usage_arguments(opts, parsed, &default_enabled, force_cards, &output) {
            Ok(pair) => pair,
            Err(early) => return early,
        };

    let mut combined = UsageCommandOutput::default();
    let mut stderr_lines = Vec::new();

    for provider in selected_providers {
        let provider_cat = crate::logging::LogCategories::provider(provider.raw_value());
        crate::logging::logger(&provider_cat).debug(
            if force_cards {
                "Rendering provider cards"
            } else {
                "Rendering provider usage"
            },
            Some(std::collections::BTreeMap::from([(
                "provider",
                provider.raw_value().to_owned(),
            )])),
        );
        let provider_snap = snapshot
            .providers
            .iter()
            .find(|p| p.id.as_str() == provider.raw_value());
        let single_out = build_provider_output(
            provider,
            provider_snap,
            &snapshot.generated_at.to_string(),
            opts,
            use_color,
            reset_style,
            now,
            &output,
            &mut stderr_lines,
        );
        combined.merge(single_out);
    }

    let mut stdout = String::new();
    if output.uses_json_output() {
        if !combined.payload.is_empty() {
            stdout = format_provider_payloads(&combined.payload, &output);
        }
    } else if use_brief {
        if !combined.brief_rows.is_empty() || !combined.card_failures.is_empty() {
            stdout = CLICardsBriefRenderer::render(
                &combined.brief_rows,
                &combined.card_failures,
                terminal.columns,
                use_color,
                enhanced,
                now,
            );
        }
    } else if use_cards {
        if !combined.cards.is_empty() || !combined.card_failures.is_empty() {
            stdout = CLICardsRenderer::render(
                &combined.cards,
                &combined.card_failures,
                terminal.columns,
                use_color,
                enhanced,
            );
        }
    } else if !combined.sections.is_empty() {
        stdout = combined.sections.join("\n\n");
    }

    RenderedCommandResult {
        stdout,
        stderr: stderr_lines.join("\n"),
        exit_code: combined.exit_code.raw_value(),
    }
}

#[allow(clippy::too_many_arguments)]
fn build_provider_output(
    provider: CliProvider,
    provider_snap: Option<&ProviderSnapshot>,
    fallback_updated_at: &str,
    opts: &UsageOptions,
    use_color: bool,
    reset_style: ResetTimeDisplayStyle,
    now: Timestamp,
    output: &CLIOutputPreferences,
    stderr_lines: &mut Vec<String>,
) -> UsageCommandOutput {
    let mut result = UsageCommandOutput::default();
    let Some(snap) = provider_snap else {
        let message = format!(
            "Provider '{}' is not available in daemon snapshot.",
            provider.raw_value()
        );
        result.exit_code = ExitCode::Failure;
        result
            .card_failures
            .push(CLICardFailure::new(provider, None, message.clone()));
        if output.uses_json_output() {
            result.payload.push(ProviderPayload {
                provider: provider.raw_value().to_string(),
                account: None,
                version: None,
                source: "auto".to_string(),
                status: None,
                usage: None,
                rate_window_labels: None,
                credits: None,
                reset_credits: None,
                antigravity_plan_info: None,
                openai_dashboard: None,
                diagnostic: None,
                error: Some(ProviderErrorPayload::new(
                    ExitCode::Failure.raw_value(),
                    message,
                    Some(CliErrorKind::Provider),
                )),
                pace: None,
                age_seconds: None,
            });
        } else {
            stderr_lines.push(format!("Error: {message}"));
        }
        return result;
    };

    if provider == CliProvider::Claude
        && (snap.source_mode == "claude-swap" || snap.source_mode == "ccs")
        && !snap.accounts.is_empty()
    {
        let swap_snapshots: Vec<ProviderAccountUsageSnapshot> = snap
            .accounts
            .iter()
            .map(|acc| {
                let mut acc_snap = ProviderSnapshot::new(
                    snap.id.clone(),
                    snap.display_name.clone(),
                    snap.enabled,
                    snap.source_mode.clone(),
                );
                acc_snap.windows = acc.windows.clone();
                acc_snap.credits = acc.credits.clone();
                acc_snap.cost = acc.cost.clone();
                acc_snap.cost_history = acc.cost_history.clone();
                acc_snap.identity = acc.identity.clone();
                acc_snap.status = acc.status.clone();
                acc_snap.utilization = acc.utilization.clone();
                acc_snap.subscription = acc.subscription.clone();
                acc_snap.last_error = acc.last_error.clone();
                acc_snap.updated_at = acc.updated_at;
                let usage = if acc.last_error.is_some()
                    && acc.windows.primary.is_none()
                    && acc.windows.secondary.is_none()
                {
                    None
                } else {
                    Some(CliUsageSnapshot::from_provider_snapshot(
                        &acc_snap,
                        fallback_updated_at,
                    ))
                };
                let email = acc
                    .identity
                    .as_ref()
                    .and_then(|m| m.value.as_ref())
                    .and_then(|id| id.account_email.clone());
                ProviderAccountUsageSnapshot::new(
                    ProviderAccountIdentity::new(&snap.source_mode, acc.id.as_str()),
                    CliProvider::Claude,
                    email.clone().unwrap_or_else(|| acc.label.clone()),
                    email.clone(),
                    email,
                    acc.active,
                    false,
                    false,
                    usage,
                    acc.last_error.as_ref().map(|e| e.message.clone()),
                    snap.source_mode.clone(),
                )
            })
            .collect();

        let fallback_status = if opts.status {
            snap.status
                .as_ref()
                .and_then(|m| m.value.as_ref())
                .map(ProviderStatusPayload::from)
        } else {
            None
        };
        let swap_opts = CLIClaudeSwapCardsRenderOptions {
            status: fallback_status,
            use_color,
            reset_style,
            weekly_work_days: None,
            now,
        };
        let swap_cards = CLIClaudeSwapCards::make_cards(&swap_snapshots, &swap_opts);
        result
            .brief_rows
            .extend(CLICardsBriefRenderer::make_rows(&swap_cards));
        result.cards.extend(swap_cards);
        result
            .sections
            .extend(CLIClaudeSwapCards::make_text_sections(
                &swap_snapshots,
                &swap_opts,
            ));
        if output.uses_json_output() {
            result.payload.extend(CLIClaudeSwapCards::make_payloads(
                &swap_snapshots,
                None,
                &swap_opts,
            ));
        }
        return result;
    }

    if opts.all_accounts && !snap.accounts.is_empty() {
        for acc in &snap.accounts {
            let mut acc_snap = ProviderSnapshot::new(
                snap.id.clone(),
                snap.display_name.clone(),
                snap.enabled,
                snap.source_mode.clone(),
            );
            acc_snap.presentation = snap.presentation.clone();
            acc_snap.windows = acc.windows.clone();
            acc_snap.credits = acc.credits.clone();
            acc_snap.cost = acc.cost.clone();
            acc_snap.cost_history = acc.cost_history.clone();
            acc_snap.identity = acc.identity.clone();
            acc_snap.status = acc.status.clone().or_else(|| snap.status.clone());
            acc_snap.utilization = acc.utilization.clone();
            acc_snap.subscription = acc.subscription.clone();
            acc_snap.last_error = acc.last_error.clone();
            acc_snap.updated_at = acc.updated_at.or(snap.updated_at);

            let account_out = build_single_account_output(
                provider,
                &acc_snap,
                Some(acc.label.as_str()),
                fallback_updated_at,
                opts,
                use_color,
                reset_style,
                now,
                output,
                stderr_lines,
            );
            result.merge(account_out);
        }
        return result;
    }

    build_single_account_output(
        provider,
        snap,
        None,
        fallback_updated_at,
        opts,
        use_color,
        reset_style,
        now,
        output,
        stderr_lines,
    )
}

#[allow(clippy::too_many_arguments)]
fn build_single_account_output(
    provider: CliProvider,
    snap: &ProviderSnapshot,
    account_label_override: Option<&str>,
    fallback_updated_at: &str,
    opts: &UsageOptions,
    use_color: bool,
    reset_style: ResetTimeDisplayStyle,
    now: Timestamp,
    output: &CLIOutputPreferences,
    stderr_lines: &mut Vec<String>,
) -> UsageCommandOutput {
    let mut result = UsageCommandOutput::default();
    let explicit_account_label = account_label_override.map(str::to_string);

    if let Some(err) = snap.last_error.as_ref()
        && snap.windows.primary.is_none()
        && snap.windows.secondary.is_none()
        && snap.windows.tertiary.is_none()
        && snap.windows.extra.is_empty()
        && snap.cost.is_none()
    {
        let code = ExitCode::from_provider_error(err);
        result.exit_code = code;
        result.card_failures.push(CLICardFailure::new(
            provider,
            explicit_account_label.clone(),
            err.message.clone(),
        ));
        if output.uses_json_output() {
            result.payload.push(ProviderPayload {
                provider: provider.raw_value().to_string(),
                account: explicit_account_label,
                version: None,
                source: snap.source_mode.clone(),
                status: None,
                usage: None,
                rate_window_labels: None,
                credits: None,
                reset_credits: None,
                antigravity_plan_info: None,
                openai_dashboard: None,
                diagnostic: None,
                error: Some(ProviderErrorPayload::from_protocol_error(
                    err,
                    code.raw_value(),
                )),
                pace: None,
                age_seconds: None,
            });
        } else if let Some(label) = account_label_override {
            stderr_lines.push(format!(
                "Error ({} - {label}): {}",
                provider.raw_value(),
                err.message
            ));
        } else {
            stderr_lines.push(format!("Error: {}", err.message));
        }
        return result;
    }

    let usage = CliUsageSnapshot::from_provider_snapshot(snap, fallback_updated_at);
    let credits = if opts.no_credits {
        None
    } else {
        snap.credits
            .as_ref()
            .and_then(|m| m.value.as_ref())
            .map(|c| {
                let mut cli_cred = CliCreditsSnapshot::new(
                    c.remaining,
                    Vec::new(),
                    c.updated_at
                        .or(snap.updated_at)
                        .map(|ts| ts.to_string())
                        .unwrap_or_else(|| fallback_updated_at.to_string()),
                );
                cli_cred.balance_read_succeeded = c.balance_read_succeeded.unwrap_or(true);
                cli_cred.credits_available = c.credits_available;
                cli_cred.balance_is_workspace = c.balance_is_workspace.unwrap_or(false);
                cli_cred
            })
    };
    let status = if opts.status {
        snap.status
            .as_ref()
            .and_then(|m| m.value.as_ref())
            .map(ProviderStatusPayload::from)
    } else {
        None
    };

    let account = explicit_account_label
        .or_else(|| opts.account.clone())
        .or_else(|| resolved_account_text(provider, &usage, None));
    let header = format_header(provider, None, &snap.source_mode);
    let context = RenderContext {
        header,
        status: status.clone(),
        account_override: account.clone(),
        fallback_account_email: None,
        use_color,
        reset_style,
        notes: Vec::new(),
        weekly_work_days: None,
    };

    result.sections.push(CLIRenderer::render_text(
        provider,
        &usage,
        credits.as_ref(),
        &context,
        now,
    ));
    let card = CLICardsRenderer::make_card(CLICardBuildInput {
        provider,
        snapshot: &usage,
        credits: credits.as_ref(),
        source: &snap.source_mode,
        status: status.as_ref(),
        account_override: account.clone(),
        fallback_account_email: None,
        is_active: false,
        uses_last_known_usage: false,
        account_problem: None,
        notes: &[],
        use_color,
        reset_style,
        weekly_work_days: None,
        now,
    });
    result
        .brief_rows
        .extend(CLICardsBriefRenderer::make_rows(std::slice::from_ref(
            &card,
        )));
    result.cards.push(card);

    if output.uses_json_output() {
        result.payload.push(ProviderPayload {
            provider: provider.raw_value().to_string(),
            account,
            version: None,
            source: snap.source_mode.clone(),
            status,
            rate_window_labels: make_provider_window_labels_payload(provider, Some(&usage), now),
            reset_credits: ProviderResetCreditsPayload::make(&usage, now),
            pace: CLIRenderer::provider_pace_payload(provider, &usage, now, None),
            usage: Some(usage),
            credits,
            antigravity_plan_info: None,
            openai_dashboard: None,
            diagnostic: None,
            error: None,
            age_seconds: None,
        });
    }

    result
}

const fn cli_source_to_core(mode: CliSourceMode) -> SourceMode {
    match mode {
        CliSourceMode::Auto => SourceMode::Auto,
        CliSourceMode::Web => SourceMode::Web,
        CliSourceMode::Cli => SourceMode::Cli,
        CliSourceMode::Oauth => SourceMode::Oauth,
        CliSourceMode::Api => SourceMode::Api,
    }
}

const fn core_source_to_cli(mode: SourceMode) -> CliSourceMode {
    match mode {
        SourceMode::Auto => CliSourceMode::Auto,
        SourceMode::Web => CliSourceMode::Web,
        SourceMode::Cli => CliSourceMode::Cli,
        SourceMode::Oauth => CliSourceMode::Oauth,
        SourceMode::Api => CliSourceMode::Api,
    }
}

/// Builds the in-process [`ariadusage_engine::runtime::EngineRuntime`] for `usage`/`cards`,
/// wiring `--web-timeout` from `parsed` into
/// [`ariadusage_engine::providers::claude::ClaudeOAuthStrategyConfig::with_web_timeout_seconds`]
/// and [`ariadusage_engine::providers::codex::CodexStrategyConfig::with_web_timeout`].
pub fn build_in_process_runtime_for_usage(
    parsed: &ParsedValues,
) -> Result<std::sync::Arc<ariadusage_engine::runtime::EngineRuntime>, String> {
    let web_timeout = decode_web_timeout(parsed)?;
    crate::build_cli_runtime_with_web_timeout(web_timeout)
}

/// Runs an in-process provider fetch via `build_in_process_runtime_for_usage()`, projecting the
/// outcome into an [`EngineSnapshot`] and rendering it through [`execute_usage_from_snapshot`] so
/// every output format (`text`, `json`, `toon`, `cards`, `cards --brief`) shares a single
/// rendering pipeline.
pub fn execute_in_process_usage(
    opts: &UsageOptions,
    parsed: &ParsedValues,
    force_cards: bool,
    terminal: TerminalCapabilities,
    now: Timestamp,
) -> RenderedCommandResult {
    let output = resolve_usage_output_preferences(parsed);

    let cfg_path = match paths::config_path() {
        Ok(p) => p,
        Err(e) => return render_runtime_error(&e.to_string(), 16, &output),
    };
    let config = ConfigStore::new(cfg_path)
        .load_effective()
        .unwrap_or_else(|_| Config::new(Config::CURRENT_VERSION, Vec::new()));

    let enabled_providers: Vec<CliProvider> = config
        .providers
        .iter()
        .filter_map(|entry| match entry {
            ProviderEntry::Typed(p) if p.enabled.unwrap_or(false) => {
                CliProvider::from_core_provider_id(&p.id)
            }
            _ => None,
        })
        .collect();
    let default_enabled = if enabled_providers.is_empty() {
        vec![CliProvider::Codex, CliProvider::Claude]
    } else {
        enabled_providers
    };

    if let Err(early) =
        validate_usage_arguments(opts, parsed, &default_enabled, force_cards, &output)
    {
        return early;
    }

    let runtime = match build_in_process_runtime_for_usage(parsed) {
        Ok(rt) => rt,
        Err(e) => return render_runtime_error(&e, 16, &output),
    };

    execute_in_process_usage_with_runtime(
        &runtime,
        &config,
        opts,
        parsed,
        force_cards,
        terminal,
        now,
    )
}

/// Runs an in-process provider fetch against a caller-supplied [`EngineRuntime`] and [`Config`],
/// fetching every selected account in order when `--all-accounts` is enabled and rendering the
/// resulting [`EngineSnapshot`] through [`execute_usage_from_snapshot`].
pub fn execute_in_process_usage_with_runtime(
    runtime: &std::sync::Arc<ariadusage_engine::runtime::EngineRuntime>,
    config: &Config,
    opts: &UsageOptions,
    parsed: &ParsedValues,
    force_cards: bool,
    terminal: TerminalCapabilities,
    now: Timestamp,
) -> RenderedCommandResult {
    crate::logging::bootstrap_cli_logging(usage_logging_configuration(opts, parsed, force_cards));
    let output = resolve_usage_output_preferences(parsed);

    let enabled_providers: Vec<CliProvider> = config
        .providers
        .iter()
        .filter_map(|entry| match entry {
            ProviderEntry::Typed(p) if p.enabled.unwrap_or(false) => {
                CliProvider::from_core_provider_id(&p.id)
            }
            _ => None,
        })
        .collect();
    let default_enabled = if enabled_providers.is_empty() {
        vec![CliProvider::Codex, CliProvider::Claude]
    } else {
        enabled_providers
    };

    let (_selection, selected_providers) =
        match validate_usage_arguments(opts, parsed, &default_enabled, force_cards, &output) {
            Ok(pair) => pair,
            Err(early) => return early,
        };

    let token_selection = decode_token_account_selection(parsed).unwrap_or_default();
    let source_mode_override = decode_source_mode(parsed);
    let include_credits = matches!(output.format, OutputFormat::Json) || !opts.no_credits;
    let runtime_kind = if !force_cards && opts.app_auto_verifier {
        FetchRuntime::Daemon
    } else {
        FetchRuntime::Cli
    };

    let rt = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => return render_runtime_error(&e.to_string(), 16, &output),
    };

    let mut base_snapshot = runtime.get_snapshot();
    base_snapshot.generated_at = now;
    let mut pre_stderr_lines: Vec<String> = Vec::new();

    for provider in &selected_providers {
        let Ok(pid) = ProviderId::new(provider.raw_value()) else {
            continue;
        };
        let Some(descriptor) = find_by_id(&pid) else {
            continue;
        };
        let provider_cfg = config.provider_config(pid.as_str());
        let configured_source = provider_cfg
            .and_then(|c| c.source)
            .map_or(CliSourceMode::Auto, core_source_to_cli);

        // Optional Claude swap integration when eligible (`CLIClaudeSwapCards`)
        if *provider == CliProvider::Claude {
            let swap_enabled = provider_cfg
                .and_then(|c| c.extra.get("claudeSwapEnabled"))
                .and_then(|raw| serde_json::from_str::<bool>(raw.get()).ok())
                .unwrap_or(false);
            let exe_path = CLIClaudeSwapCards::executable_path(provider_cfg);
            if !exe_path.is_empty()
                && CLIClaudeSwapCards::is_eligible(
                    *provider,
                    swap_enabled,
                    token_selection.uses_override(),
                    source_mode_override.map(CliSourceMode::raw_value),
                )
                && let Ok(list) = rt.block_on(ClaudeSwapAccountReader::read_account_list(
                    &exe_path,
                    ClaudeSwapAccountSource::ClaudeSwap,
                ))
            {
                let show_single = CLIClaudeSwapCards::show_single_account(provider_cfg);
                let accounts = ClaudeSwapAccountProjection::account_snapshots(
                    &list,
                    ClaudeSwapAccountSource::ClaudeSwap,
                    &[],
                    now,
                );
                if ClaudeSwapAccountProjection::should_present_accounts(accounts.len(), show_single)
                    && let Some(snap_slot) =
                        base_snapshot.providers.iter_mut().find(|p| p.id == pid)
                {
                    snap_slot.source_mode = ClaudeSwapAccountSource::ClaudeSwap.label().to_string();
                    snap_slot.accounts = accounts
                        .into_iter()
                        .map(|acc| {
                            let last_error = acc.error.map(|err| ProviderError {
                                kind: ProviderErrorKind::Unknown,
                                category: ProviderErrorCategory::Unknown,
                                message: err,
                                retry_after_seconds: None,
                            });
                            AccountSnapshot {
                                id: acc.id.id(),
                                label: acc.display_label,
                                active: acc.is_active,
                                windows: ProviderWindows::default(),
                                credits: None,
                                cost: None,
                                cost_history: None,
                                identity: None,
                                status: None,
                                utilization: None,
                                subscription: None,
                                last_error,
                                updated_at: None,
                            }
                        })
                        .collect();
                    continue;
                }
            }
        }

        // Validate token-account selection against configured accounts
        let account_set = provider_cfg
            .and_then(|c| c.token_accounts.as_ref())
            .map(|ta| TokenAccountSet {
                active_index: ta.clamped_active_index(),
                accounts: ta
                    .accounts
                    .iter()
                    .map(|m| TokenAccountEntry {
                        id: m.id.clone(),
                        label: m.label.clone(),
                        token: String::new(),
                    })
                    .collect(),
            });

        let selected_accounts = match resolve_provider_token_accounts(
            *provider,
            configured_source,
            source_mode_override,
            account_set.as_ref(),
            &token_selection,
        ) {
            Ok(entries) => entries,
            Err(err_msg) => {
                if let Some(snap_slot) = base_snapshot.providers.iter_mut().find(|p| p.id == pid) {
                    snap_slot.source_mode = source_mode_override
                        .unwrap_or(configured_source)
                        .raw_value()
                        .to_string();
                    snap_slot.last_error = Some(ProviderError {
                        kind: ProviderErrorKind::MissingCredential,
                        category: ProviderErrorCategory::Auth,
                        message: err_msg,
                        retry_after_seconds: None,
                    });
                }
                continue;
            }
        };

        let core_override = source_mode_override.map(cli_source_to_core);
        if let Some(mode) = core_override
            && !descriptor.source_modes.contains(&mode)
        {
            if let Some(snap_slot) = base_snapshot.providers.iter_mut().find(|p| p.id == pid) {
                snap_slot.source_mode = source_mode_override
                    .unwrap_or(configured_source)
                    .raw_value()
                    .to_string();
                snap_slot.last_error = Some(ProviderError {
                    kind: ProviderErrorKind::Unknown,
                    category: ProviderErrorCategory::Unknown,
                    message: format!(
                        "Source '{}' is not supported for {}.",
                        source_mode_override
                            .unwrap_or(configured_source)
                            .raw_value(),
                        provider.raw_value()
                    ),
                    retry_after_seconds: None,
                });
            }
            continue;
        }

        let fetched_status = if opts.status && descriptor.supports_status_polling() {
            rt.block_on(runtime.refresh_provider_status(pid.clone()))
                .ok()
                .flatten()
        } else {
            None
        };

        let selections: Vec<Option<TokenAccountEntry>> = if selected_accounts.is_empty() {
            vec![None]
        } else {
            selected_accounts.into_iter().map(Some).collect()
        };

        let mut first_projected_snap: Option<ProviderSnapshot> = None;
        let mut account_snapshots: Vec<AccountSnapshot> = Vec::new();

        for selected_account_opt in &selections {
            let mut effective_config = config.clone();
            if let Some(selected) = selected_account_opt
                && let Some(ProviderEntry::Typed(p_cfg)) = effective_config
                    .providers
                    .iter_mut()
                    .find(|e| e.id() == pid.as_str())
                && let Some(ta) = p_cfg.token_accounts.as_mut()
                && let Some(pos) = ta.accounts.iter().position(|a| a.id == selected.id)
            {
                ta.active_index = i64::try_from(pos).unwrap_or(0);
            }

            let provider_cat = crate::logging::LogCategories::provider(pid.as_str());
            crate::logging::logger(&provider_cat).debug(
                "Fetching provider usage",
                Some(std::collections::BTreeMap::from([(
                    "provider",
                    pid.as_str().to_owned(),
                )])),
            );

            let Some((spec, _inputs, outcome)) =
                rt.block_on(runtime.fetch_provider_for_cli_with_config(
                    &pid,
                    &effective_config,
                    runtime_kind,
                    FetchInteraction::UserInitiated,
                    core_override,
                    Some(include_credits),
                ))
            else {
                continue;
            };

            if opts.antigravity_plan_debug
                && !opts.json_only
                && *provider == CliProvider::Antigravity
                && core_source_to_cli(spec.source_mode) == CliSourceMode::Auto
            {
                let cli_attempts: Vec<ProviderFetchAttempt> = outcome
                    .attempts
                    .iter()
                    .map(|a| {
                        let diag = DiagnosticFetchAttempt::from_attempt(a);
                        let kind = match diag.kind.as_str() {
                            "web" => FetchAttemptKind::Web,
                            "oauth" => FetchAttemptKind::Oauth,
                            "api" => FetchAttemptKind::ApiToken,
                            "local" => FetchAttemptKind::LocalProbe,
                            _ => FetchAttemptKind::Cli,
                        };
                        ProviderFetchAttempt::new(
                            diag.strategy_id.unwrap_or_default(),
                            kind,
                            diag.outcome != "skipped",
                            diag.error_category,
                        )
                    })
                    .collect();
                if let Some(summary) =
                    antigravity_auto_fallback_summary(*provider, CliSourceMode::Auto, &cli_attempts)
                {
                    pre_stderr_lines.push(summary);
                }
            }

            let source_mode_str = core_source_to_cli(spec.source_mode).raw_value().to_string();
            let projected_snap = match outcome.result {
                Ok(fetch_result) => {
                    crate::logging::logger(&provider_cat).debug(
                        "Fetched provider usage",
                        Some(std::collections::BTreeMap::from([(
                            "provider",
                            pid.as_str().to_owned(),
                        )])),
                    );
                    let mut usage = fetch_result.usage.scoped_to(pid.clone());
                    if let Some(acct) = &spec.selected_account {
                        usage = usage.with_account_label(&acct.label, &pid);
                    }
                    let pace = usage.secondary.as_ref().and_then(|secondary| {
                        let is_weekly = secondary.window_minutes.is_none()
                            || secondary.window_minutes
                                == Some(ariadusage_core::pace::DEFAULT_WEEKLY_WINDOW_MINUTES);
                        if !is_weekly
                            && !descriptor
                                .pace_capability
                                .supports_reset_window_pace(secondary, now)
                        {
                            return None;
                        }
                        let resolved = descriptor
                            .pace_capability
                            .resolved_reset_window_for_pace(secondary);
                        UsagePace::weekly(
                            &resolved,
                            now,
                            resolved.window_minutes,
                            None,
                            &TimeZone::system(),
                        )
                    });
                    let input = ProjectionInput {
                        provider: pid.clone(),
                        display_name: descriptor.display_name.to_string(),
                        enabled: spec.enabled,
                        source_mode: fetch_result.source_label.clone(),
                        refresh: RefreshState::Fresh,
                        kept_snapshot: false,
                        as_of: usage.updated_at,
                        source_kind: fetch_result.strategy_kind,
                        source_label: fetch_result.source_label,
                        error: None,
                        pace,
                    };
                    let mut snap = provider_snapshot(&usage, &input);
                    if let Some(st) = fetched_status.clone()
                        && let Ok(metric) = Metric::new(
                            ariadusage_protocol::metric::MetricState::Value,
                            Some(st),
                            Some(now),
                            Some(MetricSource::new(SourceKind::Web, "status")),
                            Confidence::Exact,
                            None,
                        )
                    {
                        snap.status = Some(metric);
                    }
                    snap
                }
                Err(err) => {
                    let summary = outcome
                        .attempts
                        .iter()
                        .map(|attempt| {
                            let diag = DiagnosticFetchAttempt::from_attempt(attempt);
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
                    let error_category = outcome
                        .attempts
                        .last()
                        .and_then(|attempt| {
                            DiagnosticFetchAttempt::from_attempt(attempt).error_category
                        })
                        .unwrap_or_else(|| "unknown".to_owned());
                    let logged_debug =
                        crate::logging::will_log_to_stderr(crate::logging::CliLogLevel::Debug);
                    crate::logging::logger(&provider_cat).debug(
                        &format!("Provider fetch failed: {summary}"),
                        Some(std::collections::BTreeMap::from([
                            ("provider", pid.as_str().to_owned()),
                            ("errorCategory", error_category),
                        ])),
                    );
                    if opts.verbose && !opts.json_only && !logged_debug {
                        pre_stderr_lines.push(format!("Provider fetch failed: {summary}"));
                    }

                    let mut empty = ProviderSnapshot::new(
                        pid.clone(),
                        descriptor.display_name,
                        spec.enabled,
                        &source_mode_str,
                    );
                    empty.presentation = Some(descriptor.presentation());
                    empty.last_error = Some(match &err {
                        FetchError::Classified(c) => {
                            let proj = ProjectedError::from(c);
                            let message = if c.transport == Some(TransportClass::Timeout) {
                                "Provider request timed out".to_string()
                            } else {
                                proj.category.safe_description().to_string()
                            };
                            ProviderError {
                                kind: proj.kind,
                                category: proj.category,
                                message,
                                retry_after_seconds: proj.retry_after_seconds,
                            }
                        }
                        FetchError::NoAvailableStrategy(_) => {
                            let cat = error_kind_to_category(ProviderErrorKind::MissingCredential);
                            ProviderError {
                                kind: ProviderErrorKind::MissingCredential,
                                category: cat,
                                message: cat.safe_description().to_string(),
                                retry_after_seconds: None,
                            }
                        }
                        FetchError::Cancelled => {
                            let cat = error_kind_to_category(ProviderErrorKind::Unknown);
                            ProviderError {
                                kind: ProviderErrorKind::Unknown,
                                category: cat,
                                message: cat.safe_description().to_string(),
                                retry_after_seconds: None,
                            }
                        }
                    });
                    empty
                }
            };

            if let Some(selected) = selected_account_opt {
                let is_active = account_set.as_ref().is_some_and(|set| {
                    set.accounts
                        .get(set.active_index)
                        .is_some_and(|a| a.id == selected.id)
                });
                account_snapshots.push(AccountSnapshot {
                    id: selected.id.clone(),
                    label: selected.label.clone(),
                    active: is_active,
                    windows: projected_snap.windows.clone(),
                    credits: projected_snap.credits.clone(),
                    cost: projected_snap.cost.clone(),
                    cost_history: projected_snap.cost_history.clone(),
                    identity: projected_snap.identity.clone(),
                    status: projected_snap.status.clone(),
                    utilization: projected_snap.utilization.clone(),
                    subscription: projected_snap.subscription.clone(),
                    last_error: projected_snap.last_error.clone(),
                    updated_at: projected_snap.updated_at,
                });
            }

            if first_projected_snap.is_none() {
                first_projected_snap = Some(projected_snap);
            }
        }

        if let Some(mut projected_snap) = first_projected_snap {
            if opts.all_accounts && !account_snapshots.is_empty() {
                projected_snap.accounts = account_snapshots;
            }
            if let Some(slot) = base_snapshot.providers.iter_mut().find(|p| p.id == pid) {
                *slot = projected_snap;
            } else {
                base_snapshot.providers.push(projected_snap);
            }
        }
    }

    let mut res =
        execute_usage_from_snapshot(&base_snapshot, opts, parsed, force_cards, terminal, now);
    if !pre_stderr_lines.is_empty() {
        let prefix = pre_stderr_lines.join("\n");
        if res.stderr.is_empty() {
            res.stderr = prefix;
        } else {
            res.stderr = format!("{prefix}\n{}", res.stderr);
        }
    }
    res
}

pub fn format_provider_payloads(
    payloads: &[ProviderPayload],
    output: &CLIOutputPreferences,
) -> String {
    if output.toon_requested {
        let bytes = serde_json::to_vec(payloads).unwrap_or_default();
        return ToonFormatter::encode_json_bytes(&bytes).unwrap_or_default();
    }
    if output.pretty {
        serde_json::to_string_pretty(payloads).unwrap_or_default()
    } else {
        serde_json::to_string(payloads).unwrap_or_default()
    }
}

fn render_arg_error(message: &str, output: &CLIOutputPreferences) -> RenderedCommandResult {
    render_custom_error(
        message,
        ExitCode::InvalidArguments,
        CliErrorKind::Args,
        output,
    )
}

fn render_runtime_error(
    message: &str,
    code: i32,
    output: &CLIOutputPreferences,
) -> RenderedCommandResult {
    if output.uses_json_output() {
        let stripped = message.strip_prefix("Error: ").unwrap_or(message);
        let payload = ProviderPayload {
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
            error: Some(ProviderErrorPayload::new(
                code,
                stripped,
                Some(CliErrorKind::Runtime),
            )),
            pace: None,
            age_seconds: None,
        };
        return RenderedCommandResult {
            stdout: format_provider_payloads(&[payload], output),
            stderr: String::new(),
            exit_code: code,
        };
    }
    RenderedCommandResult {
        stdout: String::new(),
        stderr: format!("ariadusage: {message}"),
        exit_code: code,
    }
}

fn render_custom_error(
    message: &str,
    code: ExitCode,
    kind: CliErrorKind,
    output: &CLIOutputPreferences,
) -> RenderedCommandResult {
    if output.uses_json_output() {
        let stripped = message.strip_prefix("Error: ").unwrap_or(message);
        let payload = ProviderPayload {
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
            error: Some(ProviderErrorPayload::new(
                code.raw_value(),
                stripped,
                Some(kind),
            )),
            pace: None,
            age_seconds: None,
        };
        return RenderedCommandResult {
            stdout: format_provider_payloads(&[payload], output),
            stderr: String::new(),
            exit_code: code.raw_value(),
        };
    }
    let _ = make_cli_error_payload(message, code, kind, output.pretty);
    RenderedCommandResult {
        stdout: String::new(),
        stderr: message.to_string(),
        exit_code: code.raw_value(),
    }
}

pub fn run_usage_or_cards_live(
    socket: Option<PathBuf>,
    opts: &UsageOptions,
    parsed: &ParsedValues,
    force_cards: bool,
) -> Result<(), i32> {
    crate::logging::bootstrap_cli_logging(usage_logging_configuration(opts, parsed, force_cards));
    let output = resolve_usage_output_preferences(parsed);
    let terminal = TerminalCapabilities::from_current_process();
    let now = Timestamp::now();
    let has_explicit_socket = socket.is_some();

    let res = if opts.has_fetch_affecting_flags() {
        execute_in_process_usage(opts, parsed, force_cards, terminal, now)
    } else {
        match fetch_daemon_snapshot(socket) {
            Ok(snapshot) => {
                execute_usage_from_snapshot(&snapshot, opts, parsed, force_cards, terminal, now)
            }
            Err((code, msg)) => {
                if !has_explicit_socket && code == 16 {
                    execute_in_process_usage(opts, parsed, force_cards, terminal, now)
                } else {
                    render_runtime_error(msg, i32::from(code), &output)
                }
            }
        }
    };

    if !res.stderr.is_empty() && !output.json_only {
        eprintln!("{}", res.stderr);
    }
    if !res.stdout.is_empty() {
        println!("{}", res.stdout);
    }
    if res.exit_code == 0 {
        Ok(())
    } else {
        Err(res.exit_code)
    }
}
