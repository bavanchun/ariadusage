// Ported from CodexBar Tests/CodexBarTests/CLIOutputTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/CLIArgumentParsingTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/CLIProviderSelectionTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/CLIProviderWindowLabelTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/CLIUsageFetchSetupTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/CLIAntigravityFallbackSummaryTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/CLIParallelUsageTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/CLIEntryTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/TokenAccountCLISelectionTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/CLICommonOptionsTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use ariadusage_cli::exit_code::{CliErrorDescriptor, ExitCode, map_cli_error};
use ariadusage_cli::format::ResetTimeDisplayStyle;
use ariadusage_cli::help::{root_help, usage_help};
use ariadusage_cli::options::{
    CLIUsageFetchSetup, CliLogLevel, CliSourceMode, CliStderrBuffer, FetchAttemptKind,
    ProviderFetchAttempt, ProviderRuntime, TokenAccountCliSelection, TokenAccountEntry,
    TokenAccountSet, UsageCommandOutput, antigravity_auto_fallback_summary,
    app_auto_verifier_argument_error, collect_usage_outputs, containing_app_version,
    current_version_from, decode_format, decode_source_mode, decode_token_account_selection,
    decode_web_timeout, effective_argv, format_header,
    holds_antigravity_cli_session_for_plan_debug, make_cli_error_payload, make_error_payload,
    next_ancestor, normalize_version, parse_cards_args, parse_usage_args,
    render_openai_web_dashboard_text, resolve_provider_token_accounts, resolved_log_level,
    should_detect_version, should_print_exit_error, simulate_cli_parse_failure,
};
use ariadusage_cli::output_preferences::{
    CLIOutputPreferences, OutputFormat, ParsedValues, resolve_usage_output_preferences,
};
use ariadusage_cli::provider_meta::{
    CliProvider, ProviderSelection, make_provider_window_labels_payload,
};
use ariadusage_cli::renderer::{CLIRenderer, RenderContext};
use ariadusage_cli::terminal::{CLITerminalBackend, CLITerminalCapabilities, TerminalCapabilities};
use ariadusage_protocol::{
    CliCreditEvent, CliCreditsSnapshot, CliErrorKind, CliProviderCostSnapshot, CliRateWindow,
    CliUsageSnapshot, OpenAiDashboardSnapshot, ProviderErrorPayload, ProviderPayload,
};
use jiff::Timestamp;

fn svec(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| (*s).to_string()).collect()
}

fn parsed_values(options: &[(&str, &[&str])], flags: &[&str]) -> ParsedValues {
    let mut opt_map = HashMap::new();
    for (k, vals) in options {
        opt_map.insert((*k).to_string(), svec(vals));
    }
    let flag_set: HashSet<String> = flags.iter().map(|s| (*s).to_string()).collect();
    ParsedValues {
        positional: Vec::new(),
        options: opt_map,
        flags: flag_set,
    }
}

// ============================================================================
// CLIOutputTests.swift
// ============================================================================

// upstream: Tests/CodexBarTests/CLIOutputTests.swift:47 output preferences json only forces JSON
#[test]
fn output_preferences_json_only_forces_json() {
    let text_with_json_only =
        CLIOutputPreferences::from_values(&parsed_values(&[], &["jsonOnly"]), false);
    assert_eq!(text_with_json_only.format, OutputFormat::Json);
    assert!(text_with_json_only.uses_json_output());
}

// upstream: Tests/CodexBarTests/CLIOutputTests.swift:54 argv bootstrap preferences recognize toon before Commander parsing
#[test]
fn argv_bootstrap_preferences_recognize_toon_before_commander_parsing() {
    let toon = CLIOutputPreferences::from_argv(&svec(&["usage", "--format", "toon"]));
    assert_eq!(toon.format, OutputFormat::Json);
    assert!(toon.toon_requested);
    assert!(toon.uses_json_output());

    let toon_equals = CLIOutputPreferences::from_argv(&svec(&["usage", "--format=toon"]));
    assert_eq!(toon_equals.format, OutputFormat::Json);
    assert!(toon_equals.toon_requested);

    let implicit_usage = CLIOutputPreferences::from_argv(&svec(&["--format", "toon"]));
    assert_eq!(implicit_usage.format, OutputFormat::Json);
    assert!(implicit_usage.toon_requested);
}

// upstream: Tests/CodexBarTests/CLIOutputTests.swift:73 explicit toon format wins over a json shortcut in argv bootstrap
#[test]
fn explicit_toon_format_wins_over_a_json_shortcut_in_argv_bootstrap() {
    let toon_after_json =
        CLIOutputPreferences::from_argv(&svec(&["usage", "--json", "--format", "toon"]));
    assert!(toon_after_json.toon_requested);

    let toon_before_json =
        CLIOutputPreferences::from_argv(&svec(&["usage", "--format", "toon", "--json"]));
    assert!(toon_before_json.toon_requested);
}

// upstream: Tests/CodexBarTests/CLIOutputTests.swift:84 a later explicit format overrides an earlier toon request
#[test]
fn a_later_explicit_format_overrides_an_earlier_toon_request() {
    let overridden =
        CLIOutputPreferences::from_argv(&svec(&["usage", "--format", "toon", "--format", "json"]));
    assert_eq!(overridden.format, OutputFormat::Json);
    assert!(!overridden.toon_requested);
}

// upstream: Tests/CodexBarTests/CLIOutputTests.swift:91 parse failure argv bootstrap matches explicit format precedence for toon plus json
#[test]
fn parse_failure_argv_bootstrap_matches_explicit_format_precedence_for_toon_plus_json() {
    let from_argv = CLIOutputPreferences::from_argv(&svec(&[
        "usage",
        "--format",
        "toon",
        "--json",
        "--bogus-flag-xyz",
    ]));
    let from_values = resolve_usage_output_preferences(&parsed_values(
        &[("format", &["toon"])],
        &["jsonShortcut"],
    ));
    assert_eq!(from_argv.format, from_values.format);
    assert_eq!(from_argv.toon_requested, from_values.toon_requested);
    assert!(from_argv.toon_requested);
}

// upstream: Tests/CodexBarTests/CLIOutputTests.swift:98 toon is recognized for usage and ignored for every other command
#[test]
fn toon_is_recognized_for_usage_and_ignored_for_every_other_command() {
    let usage_values = parsed_values(&[("format", &["toon"])], &[]);
    let usage_prefs = resolve_usage_output_preferences(&usage_values);
    assert_eq!(usage_prefs.format, OutputFormat::Json);
    assert!(usage_prefs.toon_requested);

    let other_prefs = CLIOutputPreferences::from_values(&usage_values, false);
    assert_eq!(other_prefs.format, OutputFormat::Text);
    assert!(!other_prefs.toon_requested);
    assert!(!other_prefs.uses_json_output());
}

// upstream: Tests/CodexBarTests/CLIOutputTests.swift:114 toon does not change the decoded format of non usage commands
#[test]
fn toon_does_not_change_the_decoded_format_of_non_usage_commands() {
    let plain_toon = parsed_values(&[("format", &["toon"])], &[]);
    assert_eq!(decode_format(&plain_toon), OutputFormat::Text);

    let toon_with_json_shortcut = parsed_values(&[("format", &["toon"])], &["jsonShortcut"]);
    assert_eq!(decode_format(&toon_with_json_shortcut), OutputFormat::Json);
}

// upstream: Tests/CodexBarTests/CLIOutputTests.swift:128 argv bootstrap only recognizes toon for the usage command
#[test]
fn argv_bootstrap_only_recognizes_toon_for_the_usage_command() {
    for command in [
        "cost", "diagnose", "cache", "config", "hooks", "cookie", "cards", "sessions", "serve",
    ] {
        let prefs = CLIOutputPreferences::from_argv(&svec(&[command, "--format", "toon"]));
        assert_eq!(prefs.format, OutputFormat::Text);
        assert!(!prefs.toon_requested);
        assert!(!prefs.uses_json_output());

        let equals_prefs = CLIOutputPreferences::from_argv(&svec(&[command, "--format=toon"]));
        assert_eq!(equals_prefs.format, OutputFormat::Text);
        assert!(!equals_prefs.toon_requested);
    }
}

// upstream: Tests/CodexBarTests/CLIOutputTests.swift:146 cli error payload is JSON array
#[test]
fn cli_error_payload_is_json_array() {
    let data = make_cli_error_payload("Boom", ExitCode::Failure, CliErrorKind::Args, false)
        .expect("payload");
    let array: Vec<ProviderPayload> = serde_json::from_str(&data).expect("valid json array");
    assert_eq!(array.len(), 1);
    assert_eq!(array[0].provider, "cli");
    assert_eq!(
        array[0].error.as_ref().map(|e| e.message.as_str()),
        Some("Boom")
    );
    assert_eq!(
        array[0].error.as_ref().and_then(|e| e.kind),
        Some(CliErrorKind::Args)
    );
    assert!(array[0].usage.is_none());
}

// upstream: Tests/CodexBarTests/CLIOutputTests.swift:163 exit omits generic error when command already emitted payload
#[test]
fn exit_omits_generic_error_when_command_already_emitted_payload() {
    assert!(!should_print_exit_error(ExitCode::Failure, None));
    assert!(!should_print_exit_error(ExitCode::Success, Some("Ignored")));
    assert!(should_print_exit_error(ExitCode::Failure, Some("Boom")));
}

// upstream: Tests/CodexBarTests/CLIOutputTests.swift:352 text renderer includes Claude extra usage balance
#[test]
fn text_renderer_includes_claude_extra_usage_balance() {
    let now = Timestamp::UNIX_EPOCH;
    let snapshot = CliUsageSnapshot {
        primary: Some(CliRateWindow::new(10.0, Some(300), None, None)),
        secondary: None,
        provider_cost: Some(CliProviderCostSnapshot {
            used: 5.0,
            limit: 20.0,
            currency_code: "USD".to_string(),
            period: Some("Monthly cap".to_string()),
            balance: Some(100.0),
            updated_at: now.to_string(),
            ..Default::default()
        }),
        updated_at: now.to_string(),
        ..Default::default()
    };

    let text = CLIRenderer::render_text(
        CliProvider::Claude,
        &snapshot,
        None,
        &RenderContext::new(
            "Claude (web)",
            None,
            false,
            ResetTimeDisplayStyle::Countdown,
        ),
        now,
    );

    assert!(text.contains("Extra usage balance: $100.00"));
}

// upstream: Tests/CodexBarTests/CLIOutputTests.swift:380 text renderer does not show zero cost for Claude balance only snapshot
#[test]
fn text_renderer_does_not_show_zero_cost_for_claude_balance_only_snapshot() {
    let now = Timestamp::UNIX_EPOCH;
    let snapshot = CliUsageSnapshot {
        primary: None,
        secondary: None,
        provider_cost: Some(CliProviderCostSnapshot {
            used: 0.0,
            limit: 0.0,
            currency_code: "USD".to_string(),
            period: Some("Extra usage".to_string()),
            balance: Some(100.0),
            updated_at: now.to_string(),
            ..Default::default()
        }),
        updated_at: now.to_string(),
        ..Default::default()
    };

    let text = CLIRenderer::render_text(
        CliProvider::Claude,
        &snapshot,
        None,
        &RenderContext::new(
            "Claude (web)",
            None,
            false,
            ResetTimeDisplayStyle::Countdown,
        ),
        now,
    );

    assert!(text.contains("Extra usage balance: $100.00"));
    assert!(!text.contains("Cost: 0.0 / 0.0"));
}

// upstream: Tests/CodexBarTests/CLIOutputTests.swift:409 text renderer shows Codex purchased credits without zero cost
#[test]
fn text_renderer_shows_codex_purchased_credits_without_zero_cost() {
    let now = Timestamp::UNIX_EPOCH;
    let snapshot = CliUsageSnapshot {
        primary: None,
        secondary: None,
        provider_cost: Some(CliProviderCostSnapshot {
            used: 0.0,
            limit: 0.0,
            currency_code: CliProviderCostSnapshot::CODEX_CREDITS_CURRENCY_CODE.to_string(),
            period: Some("Extra usage".to_string()),
            balance: Some(100.0),
            updated_at: now.to_string(),
            ..Default::default()
        }),
        updated_at: now.to_string(),
        ..Default::default()
    };

    let text = CLIRenderer::render_text(
        CliProvider::Codex,
        &snapshot,
        None,
        &RenderContext::new(
            "Codex (oauth)",
            None,
            false,
            ResetTimeDisplayStyle::Countdown,
        ),
        now,
    );

    assert!(text.contains("Extra usage balance: 100.0"));
    assert!(!text.contains("Cost: 0.0 / 0.0"));
}

// upstream: Tests/CodexBarTests/CLIOutputTests.swift:438 confirmed zero Codex credits never print zero cost or an old balance
#[test]
fn confirmed_zero_codex_credits_never_print_zero_cost_or_an_old_balance() {
    let now = Timestamp::UNIX_EPOCH;
    let credits = CliCreditsSnapshot::new(0.0, Vec::new(), now.to_string());
    let snapshot = CliUsageSnapshot {
        primary: None,
        secondary: None,
        updated_at: now.to_string(),
        ..Default::default()
    }
    .attaching_codex_credits(Some(&credits));

    let text = CLIRenderer::render_text(
        CliProvider::Codex,
        &snapshot,
        None,
        &RenderContext::new(
            "Codex (oauth)",
            None,
            false,
            ResetTimeDisplayStyle::Countdown,
        ),
        now,
    );

    assert!(!text.contains("Extra usage balance"));
    assert!(!text.contains("Cost:"));
}

// ============================================================================
// CLIArgumentParsingTests.swift
// ============================================================================

// upstream: Tests/CodexBarTests/CLIArgumentParsingTests.swift:8 json shortcut does not enable json logs
#[test]
fn json_shortcut_does_not_enable_json_logs() {
    let (_, parsed) = parse_usage_args(&svec(&["--json"])).expect("parse --json");
    assert!(parsed.flags.contains("jsonShortcut"));
    assert!(!parsed.flags.contains("jsonOutput"));
    assert_eq!(decode_format(&parsed), OutputFormat::Json);
}

// upstream: Tests/CodexBarTests/CLIArgumentParsingTests.swift:19 json output flag enables json logs
#[test]
fn json_output_flag_enables_json_logs() {
    let (_, parsed) = parse_usage_args(&svec(&["--json-output"])).expect("parse --json-output");
    assert!(parsed.flags.contains("jsonOutput"));
    assert!(!parsed.flags.contains("jsonShortcut"));
    assert_eq!(decode_format(&parsed), OutputFormat::Text);
}

// upstream: Tests/CodexBarTests/CLIArgumentParsingTests.swift:30 log level and verbose are parsed
#[test]
fn log_level_and_verbose_are_parsed() {
    let (_, parsed) = parse_usage_args(&svec(&["--log-level", "info", "--verbose"]))
        .expect("parse --log-level info --verbose");
    assert!(parsed.flags.contains("verbose"));
    assert_eq!(parsed.options.get("logLevel"), Some(&svec(&["info"])));
}

// upstream: Tests/CodexBarTests/CLIArgumentParsingTests.swift:40 resolved log level defaults to error
#[test]
fn resolved_log_level_defaults_to_error() {
    assert_eq!(resolved_log_level(false, None), CliLogLevel::Error);
    assert_eq!(resolved_log_level(true, None), CliLogLevel::Debug);
    assert_eq!(resolved_log_level(false, Some("info")), CliLogLevel::Info);
}

// upstream: Tests/CodexBarTests/CLIArgumentParsingTests.swift:47 format option overrides json shortcut
#[test]
fn format_option_overrides_json_shortcut() {
    let (_, parsed) = parse_usage_args(&svec(&["--json", "--format", "text"]))
        .expect("parse --json --format text");
    assert!(parsed.flags.contains("jsonShortcut"));
    assert_eq!(parsed.options.get("format"), Some(&svec(&["text"])));
    assert_eq!(decode_format(&parsed), OutputFormat::Text);
}

// upstream: Tests/CodexBarTests/CLIArgumentParsingTests.swift:58 json only enables json format
#[test]
fn json_only_enables_json_format() {
    let (_, parsed) = parse_usage_args(&svec(&["--json-only"])).expect("parse --json-only");
    assert!(parsed.flags.contains("jsonOnly"));
    assert!(!parsed.flags.contains("jsonOutput"));
    assert_eq!(decode_format(&parsed), OutputFormat::Json);
}

// upstream: Tests/CodexBarTests/CLIArgumentParsingTests.swift:69 app auto verifier flag parses through the usage signature
#[test]
fn app_auto_verifier_flag_parses_through_the_usage_signature() {
    let (_, parsed) =
        parse_usage_args(&svec(&["--app-auto-verifier"])).expect("parse --app-auto-verifier");
    assert!(parsed.flags.contains("appAutoVerifier"));
}

// upstream: Tests/CodexBarTests/CLIArgumentParsingTests.swift:78 app auto verifier rejects account overrides
#[test]
fn app_auto_verifier_rejects_account_overrides() {
    for arguments in [
        &["--app-auto-verifier", "--account", "Configured"][..],
        &["--app-auto-verifier", "--account-index", "1"][..],
        &["--app-auto-verifier", "--all-accounts"][..],
    ] {
        let (_, parsed) = parse_usage_args(&svec(arguments)).expect("parse");
        let selection = decode_token_account_selection(&parsed).expect("selection");
        assert!(parsed.flags.contains("appAutoVerifier"));
        assert!(
            app_auto_verifier_argument_error(
                true,
                &[CliProvider::Claude],
                Some(CliSourceMode::Auto),
                &selection,
            )
            .is_some()
        );
    }
}

// upstream: Tests/CodexBarTests/CLIArgumentParsingTests.swift:132 Claude OAuth usage does not detect CLI version
#[test]
fn claude_oauth_usage_does_not_detect_cli_version() {
    assert!(!should_detect_version(
        CliProvider::Claude,
        FetchAttemptKind::Oauth
    ));
    assert!(should_detect_version(
        CliProvider::Claude,
        FetchAttemptKind::Cli
    ));
    assert!(should_detect_version(
        CliProvider::Codex,
        FetchAttemptKind::Oauth
    ));
}

// ============================================================================
// CLIProviderSelectionTests.swift
// ============================================================================

// partial-upstream: Tests/CodexBarTests/CLIProviderSelectionTests.swift:7 help includes gemini and all
#[test]
fn help_includes_gemini_and_all() {
    let usage = usage_help("0.0.0");
    let root = root_help("0.0.0");
    let expected_providers = [
        "--provider codex|",
        "|claude|",
        "|antigravity|",
        "|both|",
        "|all]",
    ];
    for provider in expected_providers {
        assert!(usage.contains(provider), "usage help missing {provider}");
        assert!(root.contains(provider), "root help missing {provider}");
    }
    assert!(usage.contains("--json"));
    assert!(root.contains("--json"));
    assert!(usage.contains("--json-only"));
    assert!(root.contains("--json-only"));
    assert!(usage.contains("--json-output"));
    assert!(root.contains("--json-output"));
    assert!(usage.contains("--log-level"));
    assert!(root.contains("--log-level"));
    assert!(usage.contains("--verbose"));
    assert!(root.contains("--verbose"));
    assert!(usage.contains("ariadusage usage --provider antigravity"));
    assert!(usage.contains("ariadusage usage --format json --provider all --pretty"));
    assert!(root.contains("ariadusage --provider antigravity"));
}

// upstream: Tests/CodexBarTests/CLIProviderSelectionTests.swift:69 help mentions source flag
#[test]
fn help_mentions_source_flag() {
    let usage = usage_help("0.0.0");
    let root = root_help("0.0.0");

    let tokens = |text: &str| -> Vec<String> {
        text.split(|c: char| c.is_whitespace() || matches!(c, '[' | ']' | '|' | ','))
            .filter(|s| !s.is_empty())
            .map(ToString::to_string)
            .collect()
    };

    assert!(usage.contains("--source"));
    assert!(root.contains("--source"));
    assert!(usage.contains("--web-timeout"));
    assert!(usage.contains("--web-debug-dump-html"));
    assert!(!tokens(&usage).iter().any(|t| t == "--web"));
    assert!(!tokens(&root).iter().any(|t| t == "--web"));
    assert!(!tokens(&usage).iter().any(|t| t == "--claude-source"));
    assert!(!tokens(&root).iter().any(|t| t == "--claude-source"));
}

// upstream: Tests/CodexBarTests/CLIProviderSelectionTests.swift:89 help advertises the toon output format
#[test]
fn help_advertises_the_toon_output_format() {
    let usage = usage_help("0.0.0");
    let root = root_help("0.0.0");

    assert!(usage.contains("--format text|json|toon"));
    assert!(root.contains("--format text|json|toon"));
    assert!(usage.contains("ariadusage usage --format toon"));
    assert!(root.contains("ariadusage --format toon"));
}

// partial-upstream: Tests/CodexBarTests/CLIProviderSelectionTests.swift:100 provider selection respects override
#[test]
fn provider_selection_respects_override() {
    let selection = ProviderSelection::from_raw(
        Some("antigravity"),
        &[CliProvider::Codex, CliProvider::Claude],
    )
    .unwrap();
    assert_eq!(
        selection.as_list(CliProvider::ALL),
        vec![CliProvider::Antigravity]
    );
}

// partial-upstream: Tests/CodexBarTests/CLIProviderSelectionTests.swift:106 provider selection uses enabled providers when three or more are enabled
#[test]
fn provider_selection_uses_enabled_providers_when_three_or_more_are_enabled() {
    let enabled = [
        CliProvider::Codex,
        CliProvider::Claude,
        CliProvider::Antigravity,
    ];
    let selection = ProviderSelection::from_raw(None, &enabled).unwrap();
    assert_eq!(selection.as_list(CliProvider::ALL), enabled);
}

// upstream: Tests/CodexBarTests/CLIProviderSelectionTests.swift:122 provider selection uses both for codex and claude
#[test]
fn provider_selection_uses_both_for_codex_and_claude() {
    let selection =
        ProviderSelection::from_raw(None, &[CliProvider::Codex, CliProvider::Claude]).unwrap();
    assert_eq!(
        selection.as_list(CliProvider::ALL),
        vec![CliProvider::Codex, CliProvider::Claude]
    );
}

// partial-upstream: Tests/CodexBarTests/CLIProviderSelectionTests.swift:128 provider selection uses custom for codex and gemini
#[test]
fn provider_selection_uses_custom_for_codex_and_gemini() {
    let enabled = [CliProvider::Codex, CliProvider::Antigravity];
    let selection = ProviderSelection::from_raw(None, &enabled).unwrap();
    assert_eq!(selection.as_list(CliProvider::ALL), enabled);
}

// upstream: Tests/CodexBarTests/CLIProviderSelectionTests.swift:147 provider selection honors empty enabled set
#[test]
fn provider_selection_honors_empty_enabled_set() {
    let selection = ProviderSelection::from_raw(None, &[]).unwrap();
    assert_eq!(
        selection.as_list(CliProvider::ALL),
        Vec::<CliProvider>::new()
    );
}

// ============================================================================
// CLIProviderWindowLabelTests.swift
// ============================================================================

fn make_window_label_payload(
    provider_id: &str,
    usage: Option<CliUsageSnapshot>,
) -> ProviderPayload {
    let rate_window_labels = CliProvider::from_cli_name(provider_id).and_then(|p| {
        usage
            .as_ref()
            .and_then(|u| make_provider_window_labels_payload(p, Some(u), Timestamp::UNIX_EPOCH))
    });
    ProviderPayload {
        provider: provider_id.to_string(),
        account: None,
        version: None,
        source: "fixture".to_string(),
        status: None,
        usage,
        rate_window_labels,
        credits: None,
        reset_credits: None,
        antigravity_plan_info: None,
        openai_dashboard: None,
        diagnostic: None,
        error: None,
        pace: None,
        age_seconds: None,
    }
}

// upstream: Tests/CodexBarTests/CLIProviderWindowLabelTests.swift:23 unknown providers and absent usage do not inherit built in window labels
#[test]
fn unknown_providers_and_absent_usage_do_not_inherit_built_in_window_labels() {
    let usage = CliUsageSnapshot {
        primary: Some(CliRateWindow::new(20.0, None, None, None)),
        secondary: None,
        updated_at: "1970-01-01T00:00:00Z".to_string(),
        ..Default::default()
    };
    assert!(
        make_window_label_payload("fixture-custom-provider", Some(usage))
            .rate_window_labels
            .is_none()
    );
    assert!(
        make_window_label_payload("v0", None)
            .rate_window_labels
            .is_none()
    );
}

// ============================================================================
// CLIUsageFetchSetupTests.swift
// ============================================================================

fn decode_setup(
    name: &str,
    arguments: &[&str],
    providers: &[CliProvider],
) -> Result<CLIUsageFetchSetup, String> {
    let args = svec(arguments);
    let (_, values) = if name == "cards" {
        parse_cards_args(&args)?
    } else {
        parse_usage_args(&args)?
    };
    let output = CLIOutputPreferences::from_values(&values, name == "usage");
    CLIUsageFetchSetup::new(&values, providers, output, name == "cards")
}

// partial-upstream: Tests/CodexBarTests/CLIUsageFetchSetupTests.swift:7 shared fetch defaults and explicit options survive decoding
#[test]
fn shared_fetch_defaults_and_explicit_options_survive_decoding() {
    for name in ["usage", "cards"] {
        let defaults = decode_setup(name, &[], &[CliProvider::Codex]).expect("defaults");
        assert_eq!(defaults.command.format, OutputFormat::Text);
        assert!(defaults.command.include_credits);
        assert_eq!(defaults.command.web_timeout, 60.0);
        assert!(defaults.command.source_mode_override.is_none());
        assert!(!defaults.include_status);
        assert!(!defaults.token_selection.uses_override());
        assert_eq!(defaults.command.cards_layout, name == "cards");
        assert_eq!(defaults.command.provider_runtime, ProviderRuntime::Cli);

        let explicit = decode_setup(
            name,
            &[
                "--source",
                "API",
                "--web-timeout",
                "0",
                "--no-credits",
                "--no-color",
                "--status",
                "--verbose",
                "--web-debug-dump-html",
                "--antigravity-plan-debug",
                "--account-index",
                "2",
            ],
            &[CliProvider::Claude],
        )
        .expect("explicit");
        assert_eq!(
            explicit.command.source_mode_override,
            Some(CliSourceMode::Api)
        );
        assert_eq!(explicit.command.web_timeout, 0.0);
        assert!(!explicit.command.include_credits);
        assert!(!explicit.command.use_color);
        assert!(explicit.include_status);
        assert!(explicit.command.verbose);
        assert!(explicit.command.web_debug_dump_html);
        assert!(explicit.command.antigravity_plan_debug);
        assert_eq!(explicit.token_selection.index, Some(1));

        let web = decode_setup(
            name,
            &["--source", "invalid", "--web"],
            &[CliProvider::Codex],
        )
        .expect("web");
        assert_eq!(web.command.source_mode_override, Some(CliSourceMode::Web));
    }
}

// upstream: Tests/CodexBarTests/CLIUsageFetchSetupTests.swift:40 shared validation keeps error precedence
#[test]
fn shared_validation_keeps_error_precedence() {
    for name in ["usage", "cards"] {
        let cases: &[(&[&str], &str)] = &[
            (
                &["--source", "invalid", "--web-timeout", "invalid"],
                "--source must be auto|web|cli|oauth|api.",
            ),
            (
                &["--web-timeout", "nan", "--account-index", "0"],
                "--web-timeout must be a finite, nonnegative number within the supported range.",
            ),
            (
                &["--account-index", "0", "--all-accounts"],
                "--account-index must be a positive integer.",
            ),
            (
                &["--account", "fixture", "--all-accounts"],
                "--all-accounts cannot be combined with --account or --account-index.",
            ),
        ];
        for (arguments, expected) in cases {
            let err =
                decode_setup(name, arguments, &[CliProvider::Codex]).expect_err("expected error");
            assert_eq!(err, *expected);
        }
        assert!(
            decode_setup(
                name,
                &["--all-accounts"],
                &[CliProvider::Codex, CliProvider::Claude]
            )
            .is_err()
        );
        let all =
            decode_setup(name, &["--all-accounts"], &[CliProvider::Codex]).expect("all codex");
        assert!(all.command.include_all_codex_accounts);
    }
}

// upstream: Tests/CodexBarTests/CLIUsageFetchSetupTests.swift:67 usage retains JSON credits and verifier policy
#[test]
fn usage_retains_json_credits_and_verifier_policy() {
    let json = decode_setup(
        "usage",
        &["--json", "--no-credits", "--json-only"],
        &[CliProvider::Codex],
    )
    .expect("json");
    assert_eq!(json.command.format, OutputFormat::Json);
    assert!(json.command.include_credits);
    assert!(json.command.json_only);
    assert!(!json.command.use_color);

    let verifier = decode_setup(
        "usage",
        &["--app-auto-verifier", "--source", "auto"],
        &[CliProvider::Claude],
    )
    .expect("verifier");
    assert_eq!(verifier.command.provider_runtime, ProviderRuntime::App);
    assert!(
        decode_setup(
            "usage",
            &["--app-auto-verifier", "--source", "auto"],
            &[CliProvider::Codex],
        )
        .is_err()
    );
    assert!(
        decode_setup(
            "usage",
            &["--app-auto-verifier", "--account", "fixture"],
            &[CliProvider::Claude],
        )
        .is_err()
    );
}

// ============================================================================
// CLIAntigravityFallbackSummaryTests.swift
// ============================================================================

// upstream: Tests/CodexBarTests/CLIAntigravityFallbackSummaryTests.swift:6 summary includes ordered per-source outcomes
#[test]
fn summary_includes_ordered_per_source_outcomes() {
    let attempts = vec![
        ProviderFetchAttempt::new(
            "antigravity.app-local",
            FetchAttemptKind::LocalProbe,
            true,
            Some("Antigravity quota request rejected.".to_string()),
        ),
        ProviderFetchAttempt::new("antigravity.cli-https", FetchAttemptKind::Cli, false, None),
        ProviderFetchAttempt::new(
            "antigravity.ide-local",
            FetchAttemptKind::LocalProbe,
            true,
            Some(
                "Antigravity language server not detected. Launch Antigravity and retry."
                    .to_string(),
            ),
        ),
    ];

    let summary =
        antigravity_auto_fallback_summary(CliProvider::Antigravity, CliSourceMode::Auto, &attempts);
    let expected = concat!(
        "Antigravity auto source outcomes: app: Antigravity quota request rejected.",
        " -> cli: skipped (unavailable)",
        " -> ide: Antigravity language server not detected. Launch Antigravity and retry.",
    );

    assert_eq!(summary.as_deref(), Some(expected));
}

// partial-upstream: Tests/CodexBarTests/CLIAntigravityFallbackSummaryTests.swift:39 summary is nil outside antigravity auto failures
#[test]
fn summary_is_nil_outside_antigravity_auto_failures() {
    let attempts = vec![ProviderFetchAttempt::new(
        "antigravity.cli-https",
        FetchAttemptKind::Cli,
        true,
        Some("example".to_string()),
    )];

    assert!(
        antigravity_auto_fallback_summary(CliProvider::Antigravity, CliSourceMode::Cli, &attempts)
            .is_none()
    );
    assert!(
        antigravity_auto_fallback_summary(CliProvider::Codex, CliSourceMode::Auto, &attempts)
            .is_none()
    );
    assert!(
        antigravity_auto_fallback_summary(CliProvider::Antigravity, CliSourceMode::Auto, &[])
            .is_none()
    );
}

// ============================================================================
// CLIParallelUsageTests.swift
// ============================================================================

// partial-upstream: Tests/CodexBarTests/CLIParallelUsageTests.swift:7 bounded provider fetching preserves output diagnostics and failure order
#[tokio::test]
async fn bounded_provider_fetching_preserves_output_diagnostics_and_failure_order() {
    let providers = [
        CliProvider::Codex,
        CliProvider::Claude,
        CliProvider::Antigravity,
    ];

    async fn fetch_slow(
        provider: CliProvider,
        active: Arc<AtomicUsize>,
        peak: Arc<AtomicUsize>,
        stderr: CliStderrBuffer,
    ) -> UsageCommandOutput {
        let cur = active.fetch_add(1, Ordering::SeqCst) + 1;
        peak.fetch_max(cur, Ordering::SeqCst);
        stderr.append(&format!("[{}] begin\n", provider.raw_value()));
        tokio::time::sleep(Duration::from_millis(if provider == CliProvider::Codex {
            50
        } else {
            15
        }))
        .await;
        stderr.append(&format!("[{}] end\n", provider.raw_value()));
        active.fetch_sub(1, Ordering::SeqCst);
        let code = if provider == CliProvider::Claude {
            ExitCode::ParseError
        } else if provider == CliProvider::Antigravity {
            ExitCode::Failure
        } else {
            ExitCode::Success
        };
        let payload = ProviderPayload {
            provider: provider.raw_value().to_string(),
            account: None,
            version: None,
            source: "synthetic".to_string(),
            status: None,
            usage: None,
            rate_window_labels: None,
            credits: None,
            reset_credits: None,
            antigravity_plan_info: None,
            openai_dashboard: None,
            diagnostic: None,
            error: if code == ExitCode::Success {
                None
            } else {
                Some(ProviderErrorPayload::new(
                    code.raw_value(),
                    "Synthetic failure",
                    Some(CliErrorKind::Provider),
                ))
            },
            pace: None,
            age_seconds: None,
        };
        UsageCommandOutput {
            sections: vec![provider.raw_value().to_string()],
            cards: Vec::new(),
            brief_rows: Vec::new(),
            card_failures: Vec::new(),
            payload: vec![payload],
            exit_code: code,
        }
    }

    let seq_active = Arc::new(AtomicUsize::new(0));
    let seq_peak = Arc::new(AtomicUsize::new(0));
    let sequential_diagnostics = CliStderrBuffer::new();
    let mut reference = UsageCommandOutput::default();
    for provider in providers {
        let out = fetch_slow(
            provider,
            Arc::clone(&seq_active),
            Arc::clone(&seq_peak),
            sequential_diagnostics.clone(),
        )
        .await;
        reference.merge(out);
    }

    let par_active = Arc::new(AtomicUsize::new(0));
    let par_peak = Arc::new(AtomicUsize::new(0));
    let parallel_diagnostics = CliStderrBuffer::new();
    let result = collect_usage_outputs(&providers, Some(&parallel_diagnostics), |p, buf| {
        fetch_slow(p, Arc::clone(&par_active), Arc::clone(&par_peak), buf)
    })
    .await;

    assert_eq!(seq_peak.load(Ordering::SeqCst), 1);
    assert_eq!(par_peak.load(Ordering::SeqCst), 3);
    let expected_names: Vec<String> = providers
        .iter()
        .map(|p| p.raw_value().to_string())
        .collect();
    assert_eq!(result.sections, expected_names);
    assert_eq!(result.sections, reference.sections);
    assert_eq!(
        result
            .payload
            .iter()
            .map(|p| p.provider.clone())
            .collect::<Vec<_>>(),
        expected_names
    );
    assert_eq!(
        serde_json::to_string(&result.payload).unwrap(),
        serde_json::to_string(&reference.payload).unwrap()
    );
    assert_eq!(
        parallel_diagnostics.contents(),
        sequential_diagnostics.contents()
    );
    assert_eq!(result.exit_code, ExitCode::ParseError);
    assert_eq!(result.exit_code, reference.exit_code);
}

// partial-upstream: Tests/CodexBarTests/CLIParallelUsageTests.swift:48 empty and single provider collections complete without extra work
#[tokio::test]
async fn empty_and_single_provider_collections_complete_without_extra_work() {
    let empty = collect_usage_outputs(&[], None, |_, _| async {
        panic!("Empty selection fetched a provider");
    })
    .await;
    assert!(empty.sections.is_empty());
    assert_eq!(empty.exit_code, ExitCode::Success);

    let single = collect_usage_outputs(
        &[CliProvider::Antigravity],
        None,
        |provider, _| async move {
            UsageCommandOutput {
                sections: vec![provider.raw_value().to_string()],
                ..Default::default()
            }
        },
    )
    .await;
    assert_eq!(single.sections, vec!["antigravity".to_string()]);
}

// ============================================================================
// CLIEntryTests.swift
// ============================================================================

// upstream: Tests/CodexBarTests/CLIEntryTests.swift:8 test_effectiveArgvDefaultsToUsage
#[test]
fn test_effective_argv_defaults_to_usage() {
    assert_eq!(effective_argv(&[]), svec(&["usage"]));
    assert_eq!(
        effective_argv(&svec(&["--json"])),
        svec(&["usage", "--json"])
    );
    assert_eq!(
        effective_argv(&svec(&["usage", "--json"])),
        svec(&["usage", "--json"])
    );
}

// upstream: Tests/CodexBarTests/CLIEntryTests.swift:62 test_usageCommanderParseFailureWithToonAndJSONRendersTOON
#[test]
fn test_usage_commander_parse_failure_with_toon_and_json_renders_toon() {
    let (status, stdout, stderr) = simulate_cli_parse_failure(&svec(&[
        "usage",
        "--format",
        "toon",
        "--json",
        "--bogus-flag-xyz",
    ]));
    assert_ne!(status, ExitCode::Success);
    assert!(stderr.is_empty());
    assert!(!stdout.is_empty());
    assert!(stdout.contains("- provider: cli"));
    assert!(stdout.contains("Unknown option --bogus-flag-xyz"));
    assert!(
        !stdout.starts_with("[{"),
        "TOON error output should not fall back to a JSON array literal"
    );
}

// upstream: Tests/CodexBarTests/CLIEntryTests.swift:73 test_usageCommanderParseFailureRendersTOONWhenRequested
#[test]
fn test_usage_commander_parse_failure_renders_toon_when_requested() {
    let (status, stdout, stderr) =
        simulate_cli_parse_failure(&svec(&["usage", "--bogus-flag-xyz", "--format", "toon"]));
    assert_ne!(status, ExitCode::Success);
    assert!(stderr.is_empty());
    assert!(!stdout.is_empty());
    assert!(stdout.contains("- provider: cli"));
    assert!(stdout.contains("message: Unknown option --bogus-flag-xyz"));
    assert!(
        !stdout.starts_with("[{"),
        "TOON error output should not fall back to a JSON array literal"
    );
    assert!(
        !stdout.contains("\"provider\""),
        "TOON error output should not contain JSON-quoted keys"
    );
}

// upstream: Tests/CodexBarTests/CLIEntryTests.swift:85 test_usageCommanderParseFailureWithEqualsFormatRendersTOON
#[test]
fn test_usage_commander_parse_failure_with_equals_format_renders_toon() {
    let (status, stdout, stderr) =
        simulate_cli_parse_failure(&svec(&["usage", "--bogus-flag-xyz", "--format=toon"]));
    assert_ne!(status, ExitCode::Success);
    assert!(stderr.is_empty());
    assert!(!stdout.is_empty());
    assert!(stdout.contains("Unknown option --bogus-flag-xyz"));
}

// upstream: Tests/CodexBarTests/CLIEntryTests.swift:97 test_nonUsageCommandsDoNotInheritTOONOutput
#[test]
fn test_non_usage_commands_do_not_inherit_toon_output() {
    for command in ["cost", "diagnose", "cache"] {
        let (status, stdout, stderr) =
            simulate_cli_parse_failure(&svec(&[command, "--format", "toon", "--bogus-flag-xyz"]));
        assert_ne!(status, ExitCode::Success);
        assert!(stdout.is_empty());
        assert!(!stderr.is_empty());
        assert!(stderr.contains("Unknown option --bogus-flag-xyz"));
    }
}

// upstream: Tests/CodexBarTests/CLIEntryTests.swift:216 test_decodesFormatFromOptionsAndFlags
#[test]
fn test_decodes_format_from_options_and_flags() {
    let json_option = parsed_values(&[("format", &["json"])], &[]);
    assert_eq!(decode_format(&json_option), OutputFormat::Json);

    let json_flag = parsed_values(&[], &["json"]);
    assert_eq!(decode_format(&json_flag), OutputFormat::Json);

    let text_default = parsed_values(&[], &[]);
    assert_eq!(decode_format(&text_default), OutputFormat::Text);
}

// partial-upstream: Tests/CodexBarTests/CLIEntryTests.swift:227 test_providerSelectionPrefersOverride
#[test]
fn test_provider_selection_prefers_override() {
    let selection = ProviderSelection::from_raw(
        Some("codex"),
        &[CliProvider::Claude, CliProvider::Antigravity],
    )
    .unwrap();
    assert_eq!(
        selection.as_list(CliProvider::ALL),
        vec![CliProvider::Codex]
    );
}

// upstream: Tests/CodexBarTests/CLIEntryTests.swift:232 test_normalizeVersionExtractsNumeric
#[test]
fn test_normalize_version_extracts_numeric() {
    assert_eq!(
        normalize_version(Some("codex 1.2.3 (build 4)")).as_deref(),
        Some("1.2.3")
    );
    assert_eq!(normalize_version(Some("  v2.0  ")).as_deref(), Some("2.0"));
}

// upstream: Tests/CodexBarTests/CLIEntryTests.swift:237 test_makeHeaderIncludesVersionWhenAvailable
#[test]
fn test_make_header_includes_version_when_available() {
    let header = format_header(CliProvider::Codex, Some("1.2.3"), "cli");
    assert!(header.contains("Codex"));
    assert!(header.contains("1.2.3"));
    assert!(header.contains("cli"));
}

// upstream: Tests/CodexBarTests/CLIEntryTests.swift:244 test_cliVersionFallsBackToContainingAppBundle
#[test]
fn test_cli_version_falls_back_to_containing_app_bundle() {
    let root = tempfile::tempdir().unwrap();
    let app_url = root.path().join("CodexBar.app");
    let contents_url = app_url.join("Contents");
    let helpers_url = contents_url.join("Helpers");
    std::fs::create_dir_all(&helpers_url).unwrap();

    let info_url = contents_url.join("Info.plist");
    let plist_xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0"><dict><key>CFBundleShortVersionString</key><string>9.8.7</string></dict></plist>"#;
    std::fs::write(&info_url, plist_xml).unwrap();

    let helper_url = helpers_url.join("CodexBarCLI");
    std::fs::write(&helper_url, b"").unwrap();

    assert_eq!(
        containing_app_version(&helper_url).as_deref(),
        Some("9.8.7")
    );
}

// upstream: Tests/CodexBarTests/CLIEntryTests.swift:265 test_containingAppVersionTerminatesOutsideAppBundle
#[test]
fn test_containing_app_version_terminates_outside_app_bundle() {
    let root = tempfile::tempdir().unwrap();
    let bin_url = root.path().join("bin");
    std::fs::create_dir_all(&bin_url).unwrap();
    let executable_url = bin_url.join("CodexBarCLI");
    std::fs::write(&executable_url, b"").unwrap();

    assert!(containing_app_version(&executable_url).is_none());
    assert!(containing_app_version(Path::new("/")).is_none());
}

// upstream: Tests/CodexBarTests/CLIEntryTests.swift:279 test_nextAncestorRejectsNonDecreasingParents
#[test]
fn test_next_ancestor_rejects_non_decreasing_parents() {
    let current = Path::new("/synthetic/current");
    let candidates = [
        Path::new("/distinct/sibling").to_path_buf(),
        Path::new("/synthetic/current/child").to_path_buf(),
    ];

    for candidate in candidates {
        let mut calls = 0;
        let ancestor = next_ancestor(current, |_| {
            calls += 1;
            candidate.clone()
        });
        assert!(ancestor.is_none());
        assert_eq!(calls, 1);
    }
}

// upstream: Tests/CodexBarTests/CLIEntryTests.swift:298 test_cliVersionFollowsSymlinkedHelper
#[cfg(unix)]
#[test]
fn test_cli_version_follows_symlinked_helper() {
    let root = tempfile::tempdir().unwrap();
    let app_url = root.path().join("CodexBar.app");
    let contents_url = app_url.join("Contents");
    let helpers_url = contents_url.join("Helpers");
    let bin_url = root.path().join("bin");
    std::fs::create_dir_all(&helpers_url).unwrap();
    std::fs::create_dir_all(&bin_url).unwrap();

    let info_url = contents_url.join("Info.plist");
    let plist_xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0"><dict><key>CFBundleShortVersionString</key><string>2.4.6</string></dict></plist>"#;
    std::fs::write(&info_url, plist_xml).unwrap();

    let helper_url = helpers_url.join("CodexBarCLI");
    std::fs::write(&helper_url, b"").unwrap();

    let symlink_url = bin_url.join("codexbar");
    std::os::unix::fs::symlink(&helper_url, &symlink_url).unwrap();

    assert_eq!(
        current_version_from(None, Some(&symlink_url), "0.0.0"),
        "2.4.6"
    );
}

// upstream: Tests/CodexBarTests/CLIEntryTests.swift:324 test_cliVersionFallsBackToAdjacentVersionFile
#[test]
fn test_cli_version_falls_back_to_adjacent_version_file() {
    for (raw, expected) in [
        ("v3.2.1\n", "3.2.1"),
        ("3.2.2\n", "3.2.2"),
        ("version-3.2.3\n", "version-3.2.3"),
    ] {
        let root = tempfile::tempdir().unwrap();
        let bin_url = root.path().join("bin");
        std::fs::create_dir_all(&bin_url).unwrap();
        let helper_url = bin_url.join("CodexBarCLI");
        std::fs::write(&helper_url, b"").unwrap();
        std::fs::write(bin_url.join("VERSION"), raw).unwrap();

        assert_eq!(
            current_version_from(None, Some(&helper_url), "0.0.0"),
            expected
        );
    }
}

// upstream: Tests/CodexBarTests/CLIEntryTests.swift:330 test_cliVersionFindsAdjacentVersionWhenInvokedViaRelativePathAndSymlink
#[test]
fn test_cli_version_finds_adjacent_version_when_invoked_via_relative_path_and_symlink() {
    let root = tempfile::tempdir().unwrap();
    let install_url = root.path().join("install/bin");
    let links_url = root.path().join("links");
    let working_dir = root.path().join("work");
    std::fs::create_dir_all(&install_url).unwrap();
    std::fs::create_dir_all(&links_url).unwrap();
    std::fs::create_dir_all(&working_dir).unwrap();

    let executable_url = install_url.join("CodexBarCLI");
    std::fs::write(&executable_url, b"").unwrap();
    std::fs::write(install_url.join("VERSION"), "8.7.6\n").unwrap();

    let relative_from_work = working_dir.join("../install/bin/CodexBarCLI");
    assert_eq!(
        format!(
            "AriadUsage {}\n",
            current_version_from(None, Some(&relative_from_work), "0.0.0")
        ),
        "AriadUsage 8.7.6\n"
    );

    #[cfg(unix)]
    {
        let symlink_url = links_url.join("ariadusage");
        std::os::unix::fs::symlink("../install/bin/CodexBarCLI", &symlink_url).unwrap();
        assert_eq!(
            format!(
                "AriadUsage {}\n",
                current_version_from(None, Some(&symlink_url), "0.0.0")
            ),
            "AriadUsage 8.7.6\n"
        );
    }
}

// upstream: Tests/CodexBarTests/CLIEntryTests.swift:370 test_cliVersionPrefersAdjacentVersionOverStandaloneBundleName
#[test]
fn test_cli_version_prefers_adjacent_version_over_standalone_bundle_name() {
    let root = tempfile::tempdir().unwrap();
    let bin_url = root.path().join("bin");
    std::fs::create_dir_all(&bin_url).unwrap();
    let helper_url = bin_url.join("CodexBarCLI");
    std::fs::write(&helper_url, b"").unwrap();
    std::fs::write(bin_url.join("VERSION"), "4.5.6\n").unwrap();

    assert_eq!(
        current_version_from(Some("CodexBar"), Some(&helper_url), "0.0.0"),
        "4.5.6"
    );
    // Rust-added: AriadUsage bundle name also prefers adjacent VERSION file
    assert_eq!(
        current_version_from(Some("AriadUsage"), Some(&helper_url), "0.0.0"),
        "4.5.6"
    );
}

// upstream: Tests/CodexBarTests/CLIEntryTests.swift:452 test_renderOpenAIWebDashboardTextIncludesSummary
#[test]
fn test_render_openai_web_dashboard_text_includes_summary() {
    let now = Timestamp::from_second(1_700_000_000).unwrap();
    let event = CliCreditEvent {
        id: "evt-1".to_string(),
        date: "Nov 14, 2023".to_string(),
        service: "codex".to_string(),
        credits_used: 10.0,
    };
    let snapshot = OpenAiDashboardSnapshot {
        signed_in_email: Some("user@example.com".to_string()),
        code_review_remaining_percent: Some(45.0),
        code_review_limit: Some(CliRateWindow::new(
            55.0,
            None,
            Some((now + Duration::from_secs(3600)).to_string()),
            None,
        )),
        credit_events: vec![event],
        daily_breakdown: Vec::new(),
        usage_breakdown: Vec::new(),
        credits_purchase_url: None,
        primary_limit: None,
        secondary_limit: None,
        credits_remaining: None,
        account_plan: None,
        updated_at: now.to_string(),
        credits_unlimited: None,
    };

    let text = render_openai_web_dashboard_text(&snapshot, now);
    assert!(text.contains("Web session: user@example.com"));
    assert!(text.contains("Code review: 45% remaining (Resets in "));
    assert!(text.contains("Web history: 1 events"));
}

// upstream: Tests/CodexBarTests/CLIEntryTests.swift:478 test_mapsErrorsToExitCodes
#[test]
fn test_maps_errors_to_exit_codes() {
    assert_eq!(
        map_cli_error(CliErrorDescriptor::BinaryNotFound),
        ExitCode::BinaryNotFound
    );
    assert_eq!(
        map_cli_error(CliErrorDescriptor::TimedOut),
        ExitCode::Timeout
    );
    assert_eq!(
        map_cli_error(CliErrorDescriptor::ParseFailed),
        ExitCode::ParseError
    );
    assert_eq!(
        map_cli_error(CliErrorDescriptor::NoRateLimitsFound),
        ExitCode::ParseError
    );
}

// upstream: Tests/CodexBarTests/CLIEntryTests.swift:485 test_antigravityPlanDebugKeepsOneShotHelperAliveUntilDebugFetch
#[test]
fn test_antigravity_plan_debug_keeps_one_shot_helper_alive_until_debug_fetch() {
    assert!(holds_antigravity_cli_session_for_plan_debug(
        CliProvider::Antigravity,
        true,
        false,
        false
    ));
    assert!(!holds_antigravity_cli_session_for_plan_debug(
        CliProvider::Codex,
        true,
        false,
        false
    ));
    assert!(!holds_antigravity_cli_session_for_plan_debug(
        CliProvider::Antigravity,
        true,
        true,
        false
    ));
    assert!(!holds_antigravity_cli_session_for_plan_debug(
        CliProvider::Antigravity,
        true,
        false,
        true
    ));
}

// upstream: Tests/CodexBarTests/CLIEntryTests.swift:508 test_missingCodexBinaryErrorPayloadUsesInstallGuidance
#[test]
fn test_missing_codex_binary_error_payload_uses_install_guidance() {
    let payload = make_error_payload(
        ExitCode::BinaryNotFound,
        "Codex CLI missing. Install via npm (`npm i -g @openai/codex`).",
        CliErrorKind::Provider,
    );
    assert_eq!(payload.code, ExitCode::BinaryNotFound.raw_value());
    assert!(payload.message.contains("Codex CLI missing"));
    assert!(!payload.message.contains("Codex not running"));
}

// upstream: Tests/CodexBarTests/CLIEntryTests.swift:516 test_providerSelectionFallsBackToBothForPrimaryPair
#[test]
fn test_provider_selection_falls_back_to_both_for_primary_pair() {
    let selection =
        ProviderSelection::from_raw(None, &[CliProvider::Codex, CliProvider::Claude]).unwrap();
    assert_eq!(selection, ProviderSelection::Both);
}

// partial-upstream: Tests/CodexBarTests/CLIEntryTests.swift:526 test_providerSelectionFallsBackToCustomWhenNonPrimary
#[test]
fn test_provider_selection_falls_back_to_custom_when_non_primary() {
    let selection =
        ProviderSelection::from_raw(None, &[CliProvider::Codex, CliProvider::Antigravity]).unwrap();
    assert_eq!(
        selection,
        ProviderSelection::Custom(vec![CliProvider::Codex, CliProvider::Antigravity])
    );
    assert_eq!(
        selection.as_list(CliProvider::ALL),
        vec![CliProvider::Codex, CliProvider::Antigravity]
    );
}

// upstream: Tests/CodexBarTests/CLIEntryTests.swift:536 test_providerSelectionHonorsEmptyEnabledSet
#[test]
fn test_provider_selection_honors_empty_enabled_set() {
    let selection = ProviderSelection::from_raw(None, &[]).unwrap();
    assert_eq!(selection, ProviderSelection::Custom(Vec::new()));
    assert_eq!(
        selection.as_list(CliProvider::ALL),
        Vec::<CliProvider>::new()
    );
}

// upstream: Tests/CodexBarTests/CLIEntryTests.swift:546 test_decodesSourceAndTimeoutOptions
#[test]
fn test_decodes_source_and_timeout_options() {
    let (_, parsed) = parse_usage_args(&svec(&["--web-timeout", "45", "--source", "oauth"]))
        .expect("parse timeout and source");
    assert_eq!(decode_web_timeout(&parsed).unwrap(), Some(45.0));
    assert_eq!(decode_source_mode(&parsed), Some(CliSourceMode::Oauth));

    let (_, parsed_web) = parse_usage_args(&svec(&["--web"])).expect("parse --web");
    assert_eq!(decode_source_mode(&parsed_web), Some(CliSourceMode::Web));
}

// upstream: Tests/CodexBarTests/CLIEntryTests.swift:557 test_rejectsUnsafeWebTimeoutOptions
#[test]
fn test_rejects_unsafe_web_timeout_options() {
    for value in ["-1", "nan", "inf", "1e300"] {
        let parsed = parsed_values(&[("webTimeout", &[value])], &[]);
        assert!(decode_web_timeout(&parsed).is_err());
    }
}

// upstream: Tests/CodexBarTests/CLIEntryTests.swift:564 test_shouldUseColorRespectsFormatAndFlags
#[test]
fn test_should_use_color_respects_format_and_flags() {
    assert!(!CLIOutputPreferences::should_use_color(
        true,
        OutputFormat::Text,
        true
    ));
    assert!(!CLIOutputPreferences::should_use_color(
        false,
        OutputFormat::Json,
        true
    ));
}

// ============================================================================
// TokenAccountCLISelectionTests.swift
// ============================================================================

fn primary_account_set() -> TokenAccountSet {
    TokenAccountSet {
        active_index: 0,
        accounts: vec![TokenAccountEntry {
            id: "00000000-0000-0000-0000-000000000001".to_string(),
            label: "Primary".to_string(),
            token: "fixture-token".to_string(),
        }],
    }
}

fn account_overrides() -> Vec<TokenAccountCliSelection> {
    vec![
        TokenAccountCliSelection::new(Some("Primary".to_string()), None, false),
        TokenAccountCliSelection::new(None, Some(0), false),
        TokenAccountCliSelection::new(None, None, true),
    ]
}

// partial-upstream: Tests/CodexBarTests/TokenAccountCLISelectionTests.swift:64 usage and cards share provider selection constraints
#[test]
fn usage_and_cards_share_provider_selection_constraints() {
    let all = TokenAccountCliSelection::new(None, None, true);
    assert_eq!(all.provider_selection_error(&[CliProvider::Codex]), None);
    assert_eq!(all.provider_selection_error(&[CliProvider::Claude]), None);
    assert_eq!(
        all.provider_selection_error(&[CliProvider::Antigravity]),
        None
    );
    assert_eq!(
        all.provider_selection_error(&[CliProvider::Codex, CliProvider::Claude]),
        Some("account selection requires a single provider.".to_string())
    );
    assert_eq!(
        all.provider_selection_error(&[]),
        Some("account selection requires a single provider.".to_string())
    );
    let automatic = TokenAccountCliSelection::new(None, None, false);
    assert_eq!(
        automatic.provider_selection_error(&[CliProvider::Codex, CliProvider::Claude]),
        None
    );
}

// upstream: Tests/CodexBarTests/TokenAccountCLISelectionTests.swift:76 antigravity CLI leaves saved OAuth accounts passive
#[test]
fn antigravity_cli_leaves_saved_oauth_accounts_passive() {
    let set = primary_account_set();
    let automatic = TokenAccountCliSelection::new(None, None, false);
    assert!(
        resolve_provider_token_accounts(
            CliProvider::Antigravity,
            CliSourceMode::Auto,
            Some(CliSourceMode::Cli),
            Some(&set),
            &automatic,
        )
        .unwrap()
        .is_empty()
    );
    assert!(
        resolve_provider_token_accounts(
            CliProvider::Antigravity,
            CliSourceMode::Cli,
            None,
            Some(&set),
            &automatic,
        )
        .unwrap()
        .is_empty()
    );
}

// upstream: Tests/CodexBarTests/TokenAccountCLISelectionTests.swift:84 antigravity explicit CLI rejects every saved account override
#[test]
fn antigravity_explicit_cli_rejects_every_saved_account_override() {
    let set = primary_account_set();
    for selection in account_overrides() {
        let err = resolve_provider_token_accounts(
            CliProvider::Antigravity,
            CliSourceMode::Auto,
            Some(CliSourceMode::Cli),
            Some(&set),
            &selection,
        )
        .expect_err("expected conflict");
        assert!(err.contains("Antigravity CLI uses its local login"));
    }
}

// upstream: Tests/CodexBarTests/TokenAccountCLISelectionTests.swift:94 antigravity configured CLI rejects every saved account override
#[test]
fn antigravity_configured_cli_rejects_every_saved_account_override() {
    let set = primary_account_set();
    for selection in account_overrides() {
        let err = resolve_provider_token_accounts(
            CliProvider::Antigravity,
            CliSourceMode::Cli,
            None,
            Some(&set),
            &selection,
        )
        .expect_err("expected conflict");
        assert!(err.contains("Antigravity CLI uses its local login"));
    }
}

// upstream: Tests/CodexBarTests/TokenAccountCLISelectionTests.swift:104 antigravity auto and OAuth retain saved account selection
#[test]
fn antigravity_auto_and_oauth_retain_saved_account_selection() {
    let set = primary_account_set();
    let mut selections = account_overrides();
    selections.push(TokenAccountCliSelection::new(None, None, false));
    for source in [CliSourceMode::Auto, CliSourceMode::Oauth] {
        for selection in &selections {
            let accounts = resolve_provider_token_accounts(
                CliProvider::Antigravity,
                CliSourceMode::Cli,
                Some(source),
                Some(&set),
                selection,
            )
            .unwrap();
            assert_eq!(
                accounts
                    .iter()
                    .map(|a| a.label.as_str())
                    .collect::<Vec<_>>(),
                vec!["Primary"]
            );
            let configured = resolve_provider_token_accounts(
                CliProvider::Antigravity,
                source,
                None,
                Some(&set),
                selection,
            )
            .unwrap();
            assert_eq!(
                configured
                    .iter()
                    .map(|a| a.label.as_str())
                    .collect::<Vec<_>>(),
                vec!["Primary"]
            );
        }
    }
}

// upstream: Tests/CodexBarTests/TokenAccountCLISelectionTests.swift:118 other providers retain saved account overrides with CLI source
#[test]
fn other_providers_retain_saved_account_overrides_with_cli_source() {
    let set = primary_account_set();
    for selection in account_overrides() {
        let accounts = resolve_provider_token_accounts(
            CliProvider::Claude,
            CliSourceMode::Cli,
            None,
            Some(&set),
            &selection,
        )
        .unwrap();
        assert_eq!(
            accounts
                .iter()
                .map(|a| a.label.as_str())
                .collect::<Vec<_>>(),
            vec!["Primary"]
        );
    }
}

// ============================================================================
// CLICommonOptionsTests.swift
// ============================================================================

// partial-upstream: Tests/CodexBarTests/CLICommonOptionsTests.swift:66 shared fetch flags retain labels and command specific options
#[test]
fn shared_fetch_flags_retain_labels_and_command_specific_options() {
    for command in ["usage", "cards"] {
        let argv = svec(&[
            "--provider",
            "codex",
            "--account",
            "fixture",
            "--account-index",
            "2",
            "--all-accounts",
            "--source",
            "api",
            "--web-timeout",
            "12",
            "--no-credits",
            "--no-color",
            "--status",
            "--web",
            "--web-debug-dump-html",
            "--antigravity-plan-debug",
            "--log-level",
            "info",
            "-v",
        ]);
        let (_, parsed) = if command == "usage" {
            parse_usage_args(&argv).expect("parse usage")
        } else {
            parse_cards_args(&argv).expect("parse cards")
        };

        let mut expected_options = HashMap::new();
        expected_options.insert("provider".to_string(), svec(&["codex"]));
        expected_options.insert("account".to_string(), svec(&["fixture"]));
        expected_options.insert("accountIndex".to_string(), svec(&["2"]));
        expected_options.insert("source".to_string(), svec(&["api"]));
        expected_options.insert("webTimeout".to_string(), svec(&["12"]));
        expected_options.insert("logLevel".to_string(), svec(&["info"]));
        assert_eq!(parsed.options, expected_options);

        let expected_flags: HashSet<String> = [
            "allAccounts",
            "noCredits",
            "noColor",
            "status",
            "web",
            "webDebugDumpHtml",
            "antigravityPlanDebug",
            "verbose",
        ]
        .into_iter()
        .map(ToString::to_string)
        .collect();
        assert_eq!(parsed.flags, expected_flags);

        let own_flag = if command == "usage" {
            "--app-auto-verifier"
        } else {
            "--brief"
        };
        let other_flag = if command == "usage" {
            "--brief"
        } else {
            "--app-auto-verifier"
        };

        let (_, own_parsed) = if command == "usage" {
            parse_usage_args(&svec(&[own_flag])).expect("own flag")
        } else {
            parse_cards_args(&svec(&[own_flag])).expect("own flag")
        };
        assert_eq!(own_parsed.flags.len(), 1);

        let other_res = if command == "usage" {
            parse_usage_args(&svec(&[other_flag]))
        } else {
            parse_cards_args(&svec(&[other_flag]))
        };
        assert!(other_res.is_err());
    }
}

// Rust-added: verify fetch-affecting flags detection for Coordinator Decision (b)
#[test]
fn has_fetch_affecting_flags_detects_every_fetch_override() {
    let (plain, _) = parse_usage_args(&svec(&["--provider", "codex", "--json"])).unwrap();
    assert!(!plain.has_fetch_affecting_flags());

    for args in [
        &["--source", "cli"][..],
        &["--account", "work"][..],
        &["--account-index", "1"][..],
        &["--all-accounts"][..],
        &["--web"][..],
        &["--web-timeout", "30"][..],
        &["--web-debug-dump-html"][..],
        &["--antigravity-plan-debug"][..],
    ] {
        let (opts, _) = parse_usage_args(&svec(args)).expect("parse fetch-affecting flag");
        assert!(
            opts.has_fetch_affecting_flags(),
            "expected {args:?} to be recognized as fetch-affecting"
        );
    }
}

// Rust-added: adjacent version file uses .ariadusage-version and ignores .codexbar-version
#[test]
fn cli_version_reads_ariadusage_version_and_ignores_codexbar_version() {
    let root = tempfile::tempdir().unwrap();
    let bin_url = root.path().join("bin");
    std::fs::create_dir_all(&bin_url).unwrap();
    let helper_url = bin_url.join("ariadusage");
    std::fs::write(&helper_url, b"").unwrap();
    std::fs::write(bin_url.join(".codexbar-version"), "9.9.9\n").unwrap();

    assert_eq!(
        current_version_from(None, Some(&helper_url), "0.1.0"),
        "0.1.0"
    );

    std::fs::write(bin_url.join(".ariadusage-version"), "v1.4.2\n").unwrap();
    assert_eq!(
        current_version_from(None, Some(&helper_url), "0.1.0"),
        "1.4.2"
    );
}

// Rust-added: CLITerminalCapabilities::detect and TerminalCapabilities::detect_stdout
#[test]
fn cli_terminal_capabilities_detect_returns_positive_width_when_tty() {
    let empty_env = HashMap::new();
    assert_eq!(
        CLITerminalCapabilities::detect(&empty_env),
        CLITerminalBackend::Standard
    );
    let caps = TerminalCapabilities::detect_stdout();
    assert!(caps.columns > 0);
}

// Rust-added: --web-timeout reaches ClaudeOAuthStrategyConfig::with_web_timeout_seconds and CodexStrategyConfig::with_web_timeout on in-process fetch runtime
#[test]
fn web_timeout_reaches_claude_oauth_strategy_config_in_process() {
    use std::time::Duration;

    use ariadusage_core::config::{Config, ProviderConfig, ProviderEntry};
    use ariadusage_core::providers::SourceMode;
    use ariadusage_engine::brokers::secret_store::{SecretStore, service::ServiceBackend};
    use ariadusage_engine::config_store::ConfigStore;
    use ariadusage_engine::registry::ProviderRegistry;
    use ariadusage_engine::runtime::EngineRuntime;
    use ariadusage_protocol::ids::ProviderId;

    let temp = tempfile::tempdir().unwrap();
    let net = Arc::new(
        ariadusage_engine::brokers::net::NetBroker::new(
            ariadusage_engine::brokers::net::NetConfig::default(),
        )
        .unwrap(),
    );

    let default_claude_cfg = ariadusage_cli::build_cli_claude_strategy_config(
        Arc::clone(&net),
        temp.path().to_path_buf(),
        temp.path().to_path_buf(),
        None,
    );
    assert_eq!(default_claude_cfg.web_timeout_seconds, 60.0);

    let default_codex_cfg = ariadusage_cli::build_cli_codex_strategy_config(
        Arc::clone(&net),
        temp.path().to_path_buf(),
        temp.path().to_path_buf(),
        None,
    );
    assert_eq!(default_codex_cfg.web_timeout, Duration::from_secs(60));

    let (opts, parsed) =
        parse_usage_args(&svec(&["--provider", "claude", "--web-timeout", "42.5"]))
            .expect("parse --web-timeout 42.5");
    let decoded = decode_web_timeout(&parsed).expect("valid web timeout");
    assert_eq!(decoded, Some(42.5));

    let custom_claude_cfg = ariadusage_cli::build_cli_claude_strategy_config(
        Arc::clone(&net),
        temp.path().to_path_buf(),
        temp.path().to_path_buf(),
        decoded,
    );
    assert_eq!(custom_claude_cfg.web_timeout_seconds, 42.5);

    let custom_codex_cfg = ariadusage_cli::build_cli_codex_strategy_config(
        Arc::clone(&net),
        temp.path().to_path_buf(),
        temp.path().to_path_buf(),
        decoded,
    );
    assert_eq!(custom_codex_cfg.web_timeout, Duration::from_secs_f64(42.5));

    let registry = ProviderRegistry::empty().with_directories(temp.path(), temp.path());
    let runtime = Arc::new(
        EngineRuntime::new(
            ConfigStore::new(temp.path().join("config.json")),
            Arc::new(SecretStore::new(ServiceBackend::new(true))),
            Arc::clone(&net),
            registry,
        )
        .with_base_environment(std::collections::BTreeMap::new()),
    );
    ariadusage_cli::register_cli_providers_with_web_timeout(
        &runtime,
        net,
        temp.path().to_path_buf(),
        Some(temp.path().to_path_buf()),
        decoded,
    );

    let mut provider_cfg = ProviderConfig::new(ProviderId::new("claude").unwrap());
    provider_cfg.enabled = Some(true);
    provider_cfg.source = Some(SourceMode::Web);
    let config = Config::new(
        Config::CURRENT_VERSION,
        vec![ProviderEntry::Typed(provider_cfg)],
    );
    let now = Timestamp::from_second(1_700_000_000).unwrap();
    let res = ariadusage_cli::usage_command::execute_in_process_usage_with_runtime(
        &runtime,
        &config,
        &opts,
        &parsed,
        false,
        TerminalCapabilities::default(),
        now,
    );
    assert_ne!(res.exit_code, ExitCode::InvalidArguments.raw_value());
}

// Rust-added: --web-debug-dump-html is rejected with exit code 64 and a clear error message
#[test]
fn web_debug_dump_html_is_rejected_with_exit_64() {
    use ariadusage_core::config::Config;
    use ariadusage_engine::brokers::secret_store::{SecretStore, service::ServiceBackend};
    use ariadusage_engine::config_store::ConfigStore;
    use ariadusage_engine::registry::ProviderRegistry;
    use ariadusage_engine::runtime::EngineRuntime;

    let temp = tempfile::tempdir().unwrap();
    let now = Timestamp::from_second(1_700_000_000).unwrap();
    let snap = ariadusage_protocol::EngineSnapshot::new(
        now,
        300,
        ariadusage_protocol::snapshot::EngineInfo {
            version: "0.1.0".to_string(),
            refreshing: false,
        },
        Vec::new(),
    );

    let (text_opts, text_parsed) = parse_usage_args(&svec(&[
        "--provider",
        "claude",
        "--web",
        "--web-debug-dump-html",
    ]))
    .expect("parse text --web-debug-dump-html");
    let text_res = ariadusage_cli::usage_command::execute_usage_from_snapshot(
        &snap,
        &text_opts,
        &text_parsed,
        false,
        TerminalCapabilities::default(),
        now,
    );
    assert_eq!(text_res.exit_code, ExitCode::InvalidArguments.raw_value());
    assert_eq!(text_res.exit_code, 64);
    assert!(text_res.stdout.is_empty());
    assert!(
        text_res
            .stderr
            .contains("--web-debug-dump-html is not supported; AriadUsage web strategies use JSON APIs and do not parse HTML.")
    );

    let net = Arc::new(
        ariadusage_engine::brokers::net::NetBroker::new(
            ariadusage_engine::brokers::net::NetConfig::default(),
        )
        .unwrap(),
    );
    let registry = ProviderRegistry::empty().with_directories(temp.path(), temp.path());
    let runtime = Arc::new(
        EngineRuntime::new(
            ConfigStore::new(temp.path().join("config.json")),
            Arc::new(SecretStore::new(ServiceBackend::new(true))),
            net,
            registry,
        )
        .with_base_environment(std::collections::BTreeMap::new()),
    );
    let config = Config::new(Config::CURRENT_VERSION, Vec::new());

    let (json_opts, json_parsed) = parse_usage_args(&svec(&[
        "--provider",
        "claude",
        "--web",
        "--web-debug-dump-html",
        "--json",
    ]))
    .expect("parse json --web-debug-dump-html");
    let json_res = ariadusage_cli::usage_command::execute_in_process_usage_with_runtime(
        &runtime,
        &config,
        &json_opts,
        &json_parsed,
        false,
        TerminalCapabilities::default(),
        now,
    );
    assert_eq!(json_res.exit_code, 64);
    assert!(json_res.stderr.is_empty());
    let payloads: Vec<ProviderPayload> =
        serde_json::from_str(&json_res.stdout).expect("valid error JSON");
    assert_eq!(payloads.len(), 1);
    let err = payloads[0].error.as_ref().expect("error payload");
    assert_eq!(err.code, 64);
    assert_eq!(err.kind, Some(CliErrorKind::Args));
    assert!(
        err.message
            .contains("--web-debug-dump-html is not supported; AriadUsage web strategies use JSON APIs and do not parse HTML.")
    );
}

// Rust-added: CLI runtime Claude and Codex strategy configs share the runtime's ProcessRegistry and LaunchGate
#[test]
fn cli_runtime_claude_and_codex_configs_share_runtime_process_registry() {
    use ariadusage_engine::brokers::secret_store::{SecretStore, service::ServiceBackend};
    use ariadusage_engine::config_store::ConfigStore;
    use ariadusage_engine::registry::ProviderRegistry;
    use ariadusage_engine::runtime::EngineRuntime;

    let temp = tempfile::tempdir().unwrap();
    let net = Arc::new(
        ariadusage_engine::brokers::net::NetBroker::new(
            ariadusage_engine::brokers::net::NetConfig::default(),
        )
        .unwrap(),
    );
    let registry = ProviderRegistry::empty().with_directories(temp.path(), temp.path());
    let runtime = Arc::new(
        EngineRuntime::new(
            ConfigStore::new(temp.path().join("config.json")),
            Arc::new(SecretStore::new(ServiceBackend::new(true))),
            Arc::clone(&net),
            registry,
        )
        .with_base_environment(std::collections::BTreeMap::new()),
    );
    let runtime_process_registry = runtime.process_registry();
    let runtime_launch_gate = runtime.launch_gate();

    let (claude_cfg, codex_cfg) = ariadusage_cli::register_cli_providers_with_web_timeout(
        &runtime,
        net,
        temp.path().to_path_buf(),
        Some(temp.path().to_path_buf()),
        Some(42.5),
    );

    assert!(Arc::ptr_eq(
        &claude_cfg.process_registry,
        &runtime_process_registry
    ));
    assert!(Arc::ptr_eq(&claude_cfg.launch_gate, &runtime_launch_gate));
    assert!(Arc::ptr_eq(
        claude_cfg.cli_session.process_registry(),
        &runtime_process_registry
    ));
    assert!(Arc::ptr_eq(
        claude_cfg.cli_session.launch_gate(),
        &runtime_launch_gate
    ));
    assert_eq!(
        claude_cfg.cli_session.dedicated_probe_dir(),
        ariadusage_engine::providers::claude::default_dedicated_probe_working_directory(
            temp.path()
        )
    );
    assert_eq!(claude_cfg.cli_session.home_directory(), temp.path());
    assert!(Arc::ptr_eq(
        &codex_cfg.cli_fetcher.process_registry,
        &runtime_process_registry
    ));
    assert!(Arc::ptr_eq(
        &codex_cfg.cli_fetcher.core_launch_gate,
        &runtime_launch_gate
    ));
    assert!(Arc::ptr_eq(
        codex_cfg.cli_fetcher.cli_session.process_registry(),
        &runtime_process_registry
    ));
    assert!(Arc::ptr_eq(
        codex_cfg.cli_fetcher.cli_session.launch_gate(),
        &runtime_launch_gate
    ));
    assert!(Arc::ptr_eq(
        &claude_cfg.process_registry,
        &codex_cfg.cli_fetcher.process_registry
    ));
    assert!(Arc::ptr_eq(
        &claude_cfg.launch_gate,
        &codex_cfg.cli_fetcher.core_launch_gate
    ));
}
