// Rust-added tests
// Minimal smoke tests for `ariadusage cost` and `ariadusage config`.

use std::collections::BTreeMap;

use ariadusage_cli::config::{
    ConfigCommandArgs, ConfigCommonArgs, ConfigDumpArgs, ConfigProviderToggleArgs,
    ConfigSetApiKeyArgs, ConfigSetSourceArgs, ConfigSubcommand, EXIT_ARGS,
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

    let mut bad_provider_out = Vec::new();
    let bad_provider_err = run_config_command_with_store(
        ConfigCommandArgs {
            common: ConfigCommonArgs::default(),
            subcommand: Some(ConfigSubcommand::Enable(ConfigProviderToggleArgs {
                provider: Some("unknown-provider".to_owned()),
                common: ConfigCommonArgs::default(),
            })),
        },
        &store,
        &mut bad_provider_out,
    )
    .expect_err("unknown provider must fail");
    assert_eq!(bad_provider_err.code, EXIT_ARGS as u8);

    let mut bad_source_out = Vec::new();
    let bad_source_err = run_config_command_with_store(
        ConfigCommandArgs {
            common: ConfigCommonArgs::default(),
            subcommand: Some(ConfigSubcommand::SetSource(ConfigSetSourceArgs {
                provider: Some("claude".to_owned()),
                source: Some("bogus".to_owned()),
                common: ConfigCommonArgs::default(),
            })),
        },
        &store,
        &mut bad_source_out,
    )
    .expect_err("unknown source must fail");
    assert_eq!(bad_source_err.code, EXIT_ARGS as u8);
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
        env: None,
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

// Rust-added: smoke test verifying default root resolution from HOME, CODEX_HOME, and CLAUDE_CONFIG_DIR produces non-empty totals without explicit root overrides
#[test]
fn cost_default_roots_resolve_from_home_codex_home_and_claude_config_dir() {
    let dir = tempfile::tempdir().expect("tempdir");
    let home_dir = dir.path().join("home");
    let today_ts = jiff::Timestamp::now().to_string();

    // Case 1: Only HOME is set -> discovers ~/.claude/projects, ~/.config/claude/projects, and ~/.codex/sessions
    let claude_home_proj = home_dir.join(".claude/projects/demo");
    let claude_xdg_proj = home_dir.join(".config/claude/projects/xdg-demo");
    let codex_home_sess = home_dir.join(".codex/sessions/2026/04/03");
    std::fs::create_dir_all(&claude_home_proj).expect("create ~/.claude/projects/demo");
    std::fs::create_dir_all(&claude_xdg_proj).expect("create ~/.config/claude/projects/xdg-demo");
    std::fs::create_dir_all(&codex_home_sess).expect("create ~/.codex/sessions");

    let claude_line_1 = format!(
        r#"{{"type":"assistant","timestamp":"{today_ts}","message":{{"id":"msg-home-1","model":"claude-sonnet-4-5","usage":{{"input_tokens":1000,"output_tokens":500,"cache_read_input_tokens":0,"cache_creation_input_tokens":0}}}}}}"#
    );
    let claude_line_2 = format!(
        r#"{{"type":"assistant","timestamp":"{today_ts}","message":{{"id":"msg-xdg-1","model":"claude-sonnet-4-5","usage":{{"input_tokens":400,"output_tokens":100,"cache_read_input_tokens":0,"cache_creation_input_tokens":0}}}}}}"#
    );
    std::fs::write(
        claude_home_proj.join("session.jsonl"),
        format!("{claude_line_1}\n"),
    )
    .expect("write claude home fixture");
    std::fs::write(
        claude_xdg_proj.join("session.jsonl"),
        format!("{claude_line_2}\n"),
    )
    .expect("write claude xdg fixture");

    let codex_lines = format!(
        concat!(
            r#"{{"type":"session_meta","timestamp":"{ts}","payload":{{"id":"sess-home-1","cwd":"/tmp/demo"}}}}"#,
            "\n",
            r#"{{"type":"turn_context","timestamp":"{ts}","payload":{{"model":"gpt-5"}}}}"#,
            "\n",
            r#"{{"type":"event_msg","timestamp":"{ts}","payload":{{"type":"token_count","info":{{"total_token_usage":{{"input_tokens":2000,"cached_input_tokens":0,"output_tokens":400,"reasoning_output_tokens":0}},"last_token_usage":{{"input_tokens":2000,"cached_input_tokens":0,"output_tokens":400,"reasoning_output_tokens":0}}}}}}}}"#,
            "\n"
        ),
        ts = today_ts
    );
    std::fs::write(codex_home_sess.join("rollout.jsonl"), codex_lines)
        .expect("write codex home fixture");

    let both_out = assert_cmd::Command::new(env!("CARGO_BIN_EXE_ariadusage"))
        .env_clear()
        .env("HOME", &home_dir)
        .args(["cost", "--provider", "both", "--format", "json"])
        .output()
        .expect("run ariadusage cost --provider both");
    assert!(
        both_out.status.success(),
        "stderr: {:?}",
        String::from_utf8_lossy(&both_out.stderr)
    );
    let both_json: Vec<serde_json::Value> =
        serde_json::from_slice(&both_out.stdout).expect("parse both json");
    assert_eq!(both_json.len(), 2);
    assert_eq!(both_json[0]["provider"], "codex");
    assert_eq!(both_json[0]["totals"]["totalTokens"], 2_400);
    assert_eq!(both_json[1]["provider"], "claude");
    assert_eq!(both_json[1]["totals"]["totalTokens"], 2_000);

    // Verify --provider all includes codex, claude, and antigravity
    let all_out = assert_cmd::Command::new(env!("CARGO_BIN_EXE_ariadusage"))
        .env_clear()
        .env("HOME", &home_dir)
        .args(["cost", "--provider", "all", "--format", "json"])
        .output()
        .expect("run ariadusage cost --provider all");
    assert!(all_out.status.success());
    let all_json: Vec<serde_json::Value> =
        serde_json::from_slice(&all_out.stdout).expect("parse all json");
    assert_eq!(all_json.len(), 3);
    assert_eq!(all_json[0]["provider"], "codex");
    assert_eq!(all_json[1]["provider"], "claude");
    assert_eq!(all_json[2]["provider"], "antigravity");

    // Case 2: CODEX_HOME and CLAUDE_CONFIG_DIR overrides (with empty HOME)
    let empty_home = dir.path().join("empty-home");
    std::fs::create_dir_all(&empty_home).expect("create empty home");
    let custom_codex_home = dir.path().join("custom-codex");
    let custom_claude_dir = dir.path().join("custom-claude");
    std::fs::create_dir_all(custom_codex_home.join("sessions/2026/04/03"))
        .expect("create custom codex sessions");
    std::fs::create_dir_all(custom_claude_dir.join("projects/custom-proj"))
        .expect("create custom claude projects");

    let custom_claude_line = format!(
        r#"{{"type":"assistant","timestamp":"{today_ts}","message":{{"id":"msg-custom-1","model":"claude-sonnet-4-5","usage":{{"input_tokens":3000,"output_tokens":700,"cache_read_input_tokens":0,"cache_creation_input_tokens":0}}}}}}"#
    );
    std::fs::write(
        custom_claude_dir.join("projects/custom-proj/session.jsonl"),
        format!("{custom_claude_line}\n"),
    )
    .expect("write custom claude fixture");

    let custom_codex_lines = format!(
        concat!(
            r#"{{"type":"session_meta","timestamp":"{ts}","payload":{{"id":"sess-custom-1","cwd":"/tmp/custom"}}}}"#,
            "\n",
            r#"{{"type":"turn_context","timestamp":"{ts}","payload":{{"model":"gpt-5"}}}}"#,
            "\n",
            r#"{{"type":"event_msg","timestamp":"{ts}","payload":{{"type":"token_count","info":{{"total_token_usage":{{"input_tokens":5000,"cached_input_tokens":0,"output_tokens":600,"reasoning_output_tokens":0}},"last_token_usage":{{"input_tokens":5000,"cached_input_tokens":0,"output_tokens":600,"reasoning_output_tokens":0}}}}}}}}"#,
            "\n"
        ),
        ts = today_ts
    );
    std::fs::write(
        custom_codex_home.join("sessions/2026/04/03/rollout.jsonl"),
        custom_codex_lines,
    )
    .expect("write custom codex fixture");

    let custom_out = assert_cmd::Command::new(env!("CARGO_BIN_EXE_ariadusage"))
        .env_clear()
        .env("HOME", &empty_home)
        .env("CODEX_HOME", &custom_codex_home)
        .env("CLAUDE_CONFIG_DIR", &custom_claude_dir)
        .args(["cost", "--provider", "both", "--format", "json"])
        .output()
        .expect("run ariadusage cost with CODEX_HOME and CLAUDE_CONFIG_DIR");
    assert!(custom_out.status.success());
    let custom_json: Vec<serde_json::Value> =
        serde_json::from_slice(&custom_out.stdout).expect("parse custom json");
    assert_eq!(custom_json.len(), 2);
    assert_eq!(custom_json[0]["provider"], "codex");
    assert_eq!(custom_json[0]["totals"]["totalTokens"], 5_600);
    assert_eq!(custom_json[1]["provider"], "claude");
    assert_eq!(custom_json[1]["totals"]["totalTokens"], 3_700);

    // Also verify run_cost_command_with_env with only env map populated (no claude_projects_roots or codex_home)
    let mut env_map = BTreeMap::new();
    env_map.insert("HOME".to_owned(), empty_home.to_string_lossy().into_owned());
    env_map.insert(
        "CODEX_HOME".to_owned(),
        custom_codex_home.to_string_lossy().into_owned(),
    );
    env_map.insert(
        "CLAUDE_CONFIG_DIR".to_owned(),
        custom_claude_dir.to_string_lossy().into_owned(),
    );
    let env_code = run_cost_command_with_env(
        &CostCommandArgs {
            provider: Some("both".to_owned()),
            json: true,
            ..CostCommandArgs::default()
        },
        &CostRunEnvironment {
            env: Some(env_map),
            ..CostRunEnvironment::default()
        },
    );
    assert_eq!(env_code, 0);
}
