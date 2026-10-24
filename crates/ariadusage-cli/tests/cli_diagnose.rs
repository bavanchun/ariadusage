// Ported from CodexBar Tests/CodexBarTests/CLIDiagnoseCommandTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/CLIEntryTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

use std::collections::BTreeMap;
use std::process::Command;

use ariadusage_cli::{diagnose_help, diagnostic_auth_summary, write_diagnostic_export};
use ariadusage_core::config::{Config, ProviderConfig, ProviderEntry};
use ariadusage_core::pipeline::DiagnosticExport;
use ariadusage_core::providers::SourceMode;
use ariadusage_engine::config_store::ConfigStore;
use ariadusage_protocol::ProviderId;
use assert_cmd::cargo::cargo_bin;

// upstream: CLIDiagnoseCommandTests.swift:7 diagnose help describes generic JSON export
#[test]
fn diagnose_help_describes_generic_json_export() {
    let help = diagnose_help("0.0.0");

    assert!(help.contains("codexbar diagnose --provider <name|all> --format json"));
    assert!(help.contains("codexbar diagnose --provider all --format json"));
    assert!(help.contains("--redact"));
    assert!(help.contains("--output <path>"));
    assert!(help.contains("safe JSON export"));
    assert!(help.contains("raw API tokens"));
}

// upstream: CLIDiagnoseCommandTests.swift:19 diagnose output writer creates parent directories
#[test]
fn diagnose_output_writer_creates_parent_directories() {
    let root = tempfile::tempdir().expect("tempdir");
    let output = root.path().join("nested").join("diagnostic.json");

    write_diagnostic_export(
        r#"{"provider":"minimax"}"#,
        output.to_str().expect("utf8 path"),
    )
    .expect("write diagnostic export");

    let contents = std::fs::read_to_string(&output).expect("read diagnostic export");
    assert_eq!(contents, r#"{"provider":"minimax"}"#);
}

// upstream: CLIDiagnoseCommandTests.swift:40 diagnose auth mode uses settings-backed MiniMax manual cookie when env token is absent
#[test]
fn diagnose_auth_mode_uses_settings_backed_minimax_manual_cookie_when_env_token_is_absent() {
    let env = BTreeMap::new();
    let summary = diagnostic_auth_summary(
        "minimax",
        None,
        false,
        false,
        false,
        &env,
        Some(concat!("Cookie", ": session_id=demo-cookie")),
    );

    assert!(summary.configured);
    assert_eq!(summary.modes, vec!["cookie"]);
}

// upstream: CLIDiagnoseCommandTests.swift:55 diagnose auth mode keeps apiToken precedence over settings cookie
#[test]
fn diagnose_auth_mode_keeps_api_token_precedence_over_settings_cookie() {
    let mut env = BTreeMap::new();
    env.insert(
        "MINIMAX_API_TOKEN".to_owned(),
        "sk-api-demo-token".to_owned(),
    );

    let summary = diagnostic_auth_summary(
        "minimax",
        None,
        false,
        false,
        false,
        &env,
        Some(concat!("Cookie", ": session_id=demo-cookie")),
    );

    assert!(summary.configured);
    assert_eq!(summary.modes, vec!["apiToken"]);
}

// upstream: CLIDiagnoseCommandTests.swift:70 generic diagnose auth summary detects provider config
#[test]
fn generic_diagnose_auth_summary_detects_provider_config() {
    let env = BTreeMap::new();
    let summary = diagnostic_auth_summary("openai", None, true, false, false, &env, None);

    assert!(summary.configured);
    assert_eq!(summary.modes, vec!["api"]);
}

// upstream: CLIDiagnoseCommandTests.swift:83 generic diagnose auth summary detects provider environment credentials
#[test]
fn generic_diagnose_auth_summary_detects_provider_environment_credentials() {
    let mut env = BTreeMap::new();
    env.insert("OPENAI_API_KEY".to_owned(), "sk-test".to_owned());

    let summary = diagnostic_auth_summary("openai", None, false, false, false, &env, None);

    assert!(summary.configured);
    assert_eq!(summary.modes, vec!["api"]);
}

// upstream: CLIDiagnoseCommandTests.swift:96 generic diagnose auth summary detects Chutes environment credentials
#[test]
fn generic_diagnose_auth_summary_detects_chutes_environment_credentials() {
    let mut env = BTreeMap::new();
    env.insert("CHUTES_API_KEY".to_owned(), "chutes-test".to_owned());

    let summary = diagnostic_auth_summary("chutes", None, false, false, false, &env, None);

    assert!(summary.configured);
    assert_eq!(summary.modes, vec!["api"]);
}

// upstream: CLIDiagnoseCommandTests.swift:109 generic diagnose auth summary detects Neuralwatt environment credentials
#[test]
fn generic_diagnose_auth_summary_detects_neuralwatt_environment_credentials() {
    let mut env = BTreeMap::new();
    env.insert("NEURALWATT_API_KEY".to_owned(), "sk-test".to_owned());

    let summary = diagnostic_auth_summary("neuralwatt", None, false, false, false, &env, None);

    assert!(summary.configured);
    assert_eq!(summary.modes, vec!["api"]);
}

// upstream: CLIDiagnoseCommandTests.swift:122 generic diagnose auth summary requires complete Bedrock credentials
#[test]
fn generic_diagnose_auth_summary_requires_complete_bedrock_credentials() {
    let mut partial_env = BTreeMap::new();
    partial_env.insert("AWS_ACCESS_KEY_ID".to_owned(), "access-only".to_owned());
    let partial = diagnostic_auth_summary("bedrock", None, true, false, false, &partial_env, None);
    assert!(!partial.configured);
    assert!(partial.modes.is_empty());

    let mut complete_env = BTreeMap::new();
    complete_env.insert("AWS_ACCESS_KEY_ID".to_owned(), "access".to_owned());
    complete_env.insert("AWS_SECRET_ACCESS_KEY".to_owned(), "secret".to_owned());
    let complete =
        diagnostic_auth_summary("bedrock", None, false, false, false, &complete_env, None);
    assert!(complete.configured);
    assert_eq!(complete.modes, vec!["api"]);
}

// upstream: CLIDiagnoseCommandTests.swift:146 generic diagnose auth summary does not assume ambient credentials
#[test]
fn generic_diagnose_auth_summary_does_not_assume_ambient_credentials() {
    let env = BTreeMap::new();
    let summary = diagnostic_auth_summary("codex", None, false, false, false, &env, None);

    assert!(!summary.configured);
    assert!(summary.modes.is_empty());
}

// upstream: CLIEntryTests.swift:865 test_antigravityDiagnoseSerializesTerminalFailureAndLogsSafeCategory
#[test]
fn antigravity_diagnose_serializes_terminal_failure_and_logs_safe_category() {
    let temp = tempfile::tempdir().expect("tempdir");
    let root = temp.path();
    let config_path = root.join("config.json");
    let mut antigravity_cfg =
        ProviderConfig::new(ProviderId::new("antigravity").expect("antigravity id"));
    antigravity_cfg.enabled = Some(true);
    antigravity_cfg.source = Some(SourceMode::Oauth);
    ConfigStore::new(config_path.clone())
        .save(&Config::new(
            Config::CURRENT_VERSION,
            vec![ProviderEntry::Typed(antigravity_cfg)],
        ))
        .expect("save config");

    let bin = cargo_bin("ariadusage");
    let gemini_home = root.join(".gemini");
    let runtime_dir = root.join("runtime");
    std::fs::create_dir_all(&runtime_dir).expect("create runtime dir");

    let result = Command::new(&bin)
        .args([
            "diagnose",
            "--provider",
            "antigravity",
            "--format",
            "json",
            "--pretty",
        ])
        .env("HOME", root)
        .env("CFFIXED_USER_HOME", root)
        .env("CODEX_HOME", root.join(".codex"))
        .env("GEMINI_CLI_HOME", &gemini_home)
        .env("XDG_RUNTIME_DIR", &runtime_dir)
        .env("CODEXBAR_CONFIG", &config_path)
        .env("ARIADUSAGE_DISABLE_KEYRING", "1")
        .env("ANTIGRAVITY_OAUTH_CREDENTIALS_JSON", "{}")
        .output()
        .expect("run diagnose");

    assert_eq!(
        result.status.code(),
        Some(0),
        "diagnose stderr: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    let diagnostic: DiagnosticExport =
        serde_json::from_slice(&result.stdout).expect("decode DiagnosticExport");
    assert_eq!(diagnostic.source_mode, "oauth");
    assert_eq!(diagnostic.fetch_attempts.len(), 1);
    assert_eq!(
        diagnostic.fetch_attempts[0].strategy_id.as_deref(),
        Some("antigravity.oauth")
    );
    assert_eq!(diagnostic.fetch_attempts[0].outcome, "failed");
    assert_eq!(
        diagnostic.fetch_attempts[0].error_category.as_deref(),
        Some("auth")
    );

    let usage = Command::new(&bin)
        .args(["usage", "--provider", "antigravity", "--verbose"])
        .env("HOME", root)
        .env("CFFIXED_USER_HOME", root)
        .env("CODEX_HOME", root.join(".codex"))
        .env("GEMINI_CLI_HOME", &gemini_home)
        .env("XDG_RUNTIME_DIR", &runtime_dir)
        .env("CODEXBAR_CONFIG", &config_path)
        .env("ARIADUSAGE_DISABLE_KEYRING", "1")
        .env("ANTIGRAVITY_OAUTH_CREDENTIALS_JSON", "{}")
        .output()
        .expect("run usage --verbose");

    assert_ne!(usage.status.code(), Some(0));
    let stderr = String::from_utf8(usage.stderr).expect("utf8 stderr");
    assert_eq!(stderr.matches("Provider fetch failed").count(), 1);
    assert!(stderr.contains("antigravity.oauth (oauth): failed: auth"));
    let failure_log: String = stderr
        .lines()
        .filter(|line| line.contains("Provider fetch failed"))
        .collect();
    assert!(!failure_log.contains("Google auth not found"));
}

#[test]
fn diagnose_output_never_contains_secret_token_cookie_email_or_account_id() {
    let temp = tempfile::tempdir().expect("tempdir");
    let root = temp.path();
    let config_path = root.join("config.json");
    let mut claude_cfg = ProviderConfig::new(ProviderId::new("claude").expect("claude id"));
    claude_cfg.enabled = Some(true);
    claude_cfg.source = Some(SourceMode::Oauth);
    ConfigStore::new(config_path.clone())
        .save(&Config::new(
            Config::CURRENT_VERSION,
            vec![ProviderEntry::Typed(claude_cfg)],
        ))
        .expect("save config");

    let bin = cargo_bin("ariadusage");
    let runtime_dir = root.join("runtime");
    std::fs::create_dir_all(&runtime_dir).expect("create runtime dir");

    let fake_token = concat!("sk-ant-", "oat01-supersecretinventedtoken999");
    let fake_cookie = concat!("sessionKey=", "sk-ant-", "sid01-supersecretcookie999");
    let fake_email = "invented.owner@example.com";
    let fake_account = "org_invented_account_123456";

    let result = Command::new(&bin)
        .args([
            "diagnose",
            "--provider",
            "claude",
            "--format",
            "json",
            "--pretty",
        ])
        .env("HOME", root)
        .env("XDG_RUNTIME_DIR", &runtime_dir)
        .env("CODEXBAR_CONFIG", &config_path)
        .env("ARIADUSAGE_DISABLE_KEYRING", "1")
        .env("CLAUDE_CODE_OAUTH_TOKEN", fake_token)
        .env("CLAUDE_COOKIE", fake_cookie)
        .env("USER_EMAIL", fake_email)
        .env("ACCOUNT_ID", fake_account)
        .output()
        .expect("run claude diagnose");

    assert_eq!(result.status.code(), Some(0));
    let stdout = String::from_utf8(result.stdout).expect("utf8 stdout");
    assert!(!stdout.contains(fake_token));
    assert!(!stdout.contains(fake_cookie));
    assert!(!stdout.contains(fake_email));
    assert!(!stdout.contains(fake_account));
}
