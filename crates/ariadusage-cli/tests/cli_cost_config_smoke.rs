// Rust-added tests
// Minimal smoke tests for `ariadusage cost` and `ariadusage config`.
// Literal upstream test ports are owned by w-mac-5.

use ariadusage_cli::config::{
    ConfigCommandArgs, ConfigCommonArgs, ConfigDumpArgs, ConfigProviderToggleArgs,
    ConfigSetApiKeyArgs, ConfigSetSourceArgs, ConfigSubcommand,
    UNSUPPORTED_CONFIG_OPERATION_EXIT_CODE, run_config_command_with_store,
};
use ariadusage_cli::cost::{
    CostCommandArgs, CostGroupBy, CostProvider, CostReportingPeriod, CostRunEnvironment,
    CostUsageDailyReportEntry, CostUsageModelBreakdown, CostUsageTokenSnapshot,
    EXIT_COST_UNSUPPORTED, make_cost_payload, render_cost_text, run_cost_command_with_env,
};
use ariadusage_cli::hooks::HooksOutputArgs;
use ariadusage_engine::config_store::ConfigStore;
use jiff::Timestamp;
use jiff::tz::TimeZone;

// Rust-added: smoke test for config providers, enable/disable, set-source, and validate against an isolated ConfigStore
#[test]
fn config_round_trip_providers_toggle_and_set_source_in_temp_store() {
    let dir = tempfile::tempdir().expect("tempdir");
    let config_path = dir.path().join("config.json");
    let store = ConfigStore::new(config_path);

    let mut out = Vec::new();
    run_config_command_with_store(
        ConfigCommandArgs {
            common: ConfigCommonArgs::default(),
            subcommand: Some(ConfigSubcommand::Enable(ConfigProviderToggleArgs {
                provider: Some("claude".to_owned()),
                common: ConfigCommonArgs {
                    output: HooksOutputArgs {
                        json: true,
                        ..HooksOutputArgs::default()
                    },
                    ..ConfigCommonArgs::default()
                },
            })),
        },
        &store,
        &mut out,
    )
    .expect("enable claude");
    let enable_json: serde_json::Value = serde_json::from_slice(&out).expect("enable json");
    assert_eq!(enable_json["provider"], "claude");
    assert_eq!(enable_json["enabled"], true);

    out.clear();
    run_config_command_with_store(
        ConfigCommandArgs {
            common: ConfigCommonArgs::default(),
            subcommand: Some(ConfigSubcommand::SetSource(ConfigSetSourceArgs {
                provider: Some("claude".to_owned()),
                source: Some("cli".to_owned()),
                common: ConfigCommonArgs {
                    output: HooksOutputArgs {
                        json: true,
                        ..HooksOutputArgs::default()
                    },
                    ..ConfigCommonArgs::default()
                },
            })),
        },
        &store,
        &mut out,
    )
    .expect("set-source claude cli");
    let source_json: serde_json::Value = serde_json::from_slice(&out).expect("set-source json");
    assert_eq!(source_json["provider"], "claude");
    assert_eq!(source_json["source"], "cli");
    assert_eq!(source_json["enabled"], true);

    out.clear();
    run_config_command_with_store(
        ConfigCommandArgs {
            common: ConfigCommonArgs::default(),
            subcommand: Some(ConfigSubcommand::Providers(ConfigCommonArgs {
                output: HooksOutputArgs {
                    json: true,
                    ..HooksOutputArgs::default()
                },
                ..ConfigCommonArgs::default()
            })),
        },
        &store,
        &mut out,
    )
    .expect("providers json");
    let providers_json: serde_json::Value = serde_json::from_slice(&out).expect("providers list");
    let rows = providers_json.as_array().expect("providers array");
    assert!(
        rows.iter()
            .any(|r| r["provider"] == "claude" && r["enabled"] == true)
    );
}

// Rust-added: smoke test verifying config dump redacts legacy credentials and rejects --show-secrets and set-api-key with exit code 20
#[test]
fn config_dump_redacts_secrets_and_rejects_secret_operations_with_code_20() {
    let dir = tempfile::tempdir().expect("tempdir");
    let config_path = dir.path().join("config.json");
    let planted_secret = "sk-ant-planted-test-value-do-not-print";
    std::fs::write(
        &config_path,
        format!(
            r#"{{"version":1,"providers":[{{"id":"claude","enabled":true,"apiKey":"{planted_secret}"}}]}}"#
        ),
    )
    .expect("write legacy config");

    let store = ConfigStore::new(config_path);
    let mut out = Vec::new();
    run_config_command_with_store(
        ConfigCommandArgs {
            common: ConfigCommonArgs::default(),
            subcommand: Some(ConfigSubcommand::Dump(ConfigDumpArgs {
                show_secrets: false,
                common: ConfigCommonArgs::default(),
            })),
        },
        &store,
        &mut out,
    )
    .expect("dump redacted");
    let dumped = String::from_utf8(out).expect("utf8 dump");
    assert!(!dumped.contains(planted_secret));
    assert!(dumped.contains("[REDACTED]"));

    let mut show_out = Vec::new();
    let show_err = run_config_command_with_store(
        ConfigCommandArgs {
            common: ConfigCommonArgs::default(),
            subcommand: Some(ConfigSubcommand::Dump(ConfigDumpArgs {
                show_secrets: true,
                common: ConfigCommonArgs::default(),
            })),
        },
        &store,
        &mut show_out,
    )
    .expect_err("show-secrets must be rejected");
    assert_eq!(show_err.code, UNSUPPORTED_CONFIG_OPERATION_EXIT_CODE);
    assert!(show_out.is_empty());

    let mut set_key_out = Vec::new();
    let set_key_err = run_config_command_with_store(
        ConfigCommandArgs {
            common: ConfigCommonArgs::default(),
            subcommand: Some(ConfigSubcommand::SetApiKey(ConfigSetApiKeyArgs {
                provider: Some("claude".to_owned()),
                api_key: Some(planted_secret.to_owned()),
                ..ConfigSetApiKeyArgs::default()
            })),
        },
        &store,
        &mut set_key_out,
    )
    .expect_err("set-api-key must be rejected");
    assert_eq!(set_key_err.code, UNSUPPORTED_CONFIG_OPERATION_EXIT_CODE);
    assert!(
        !set_key_err
            .message
            .as_deref()
            .unwrap_or_default()
            .contains(planted_secret)
    );
}

// Rust-added: smoke test for cost text/JSON rendering and isolated local scanner invocation
#[test]
fn cost_smoke_renders_text_and_json_and_rejects_unsupported_flags_with_code_21() {
    let tz = TimeZone::UTC;
    let now: Timestamp = "2026-04-03T12:00:00Z".parse().expect("timestamp");
    let snapshot = CostUsageTokenSnapshot {
        session_tokens: Some(1_500),
        session_cost_usd: Some(0.05),
        last_30_days_tokens: Some(1_500),
        last_30_days_cost_usd: Some(0.05),
        history_days: 30,
        daily: vec![CostUsageDailyReportEntry {
            date: "2026-04-03".to_owned(),
            input_tokens: Some(1_000),
            output_tokens: Some(500),
            total_tokens: Some(1_500),
            cost_usd: Some(0.05),
            request_count: Some(1),
            priced_request_count: Some(1),
            model_breakdowns: Some(vec![CostUsageModelBreakdown::new(
                "claude-sonnet-4-5",
                Some(0.05),
                Some(1_500),
            )]),
            ..CostUsageDailyReportEntry::default()
        }],
        updated_at: now,
        ..CostUsageTokenSnapshot::default()
    }
    .reporting(CostReportingPeriod::Rolling { days: 30 });

    let text = render_cost_text(
        CostProvider::Claude,
        &snapshot,
        CostGroupBy::None,
        false,
        &tz,
        true,
    );
    assert!(text.contains("Claude Cost (API-rate estimate)"));
    assert!(text.contains("Today: $0.05"));

    let payload = make_cost_payload(CostProvider::Claude, Some(&snapshot), None, &tz);
    assert_eq!(payload.provider, "claude");
    assert_eq!(payload.source, "local");
    assert_eq!(payload.daily.len(), 1);

    let dir = tempfile::tempdir().expect("tempdir");
    let env = CostRunEnvironment {
        claude_projects_roots: Some(vec![dir.path().join("claude-projects")]),
        codex_home: Some(dir.path().join("codex-home")),
        cache_root: Some(dir.path().join("cache")),
        config_path: Some(dir.path().join("config.json")),
        now: Some(now),
        timezone: Some(tz),
        catalog: None,
    };

    let code = run_cost_command_with_env(
        &CostCommandArgs {
            provider: Some("claude".to_owned()),
            json: true,
            ..CostCommandArgs::default()
        },
        &env,
    );
    assert_eq!(code, 0);

    let unsupported_code = run_cost_command_with_env(
        &CostCommandArgs {
            remote: Some("devbox".to_owned()),
            json_only: true,
            ..CostCommandArgs::default()
        },
        &env,
    );
    assert_eq!(unsupported_code, EXIT_COST_UNSUPPORTED);
}
