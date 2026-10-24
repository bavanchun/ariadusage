// Ported from CodexBar Tests/CodexBarTests/CLIDiagnoseCommandTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/CLIEntryTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

use std::collections::BTreeMap;
use std::process::Command;

use ariadusage_cli::{diagnose_help, diagnostic_auth_summary, write_diagnostic_export};
use ariadusage_core::config::{Config, ProviderConfig, ProviderEntry};
use ariadusage_core::model::{ProviderIdentitySnapshot, UsageSnapshot};
use ariadusage_core::pipeline::{
    AttemptFailure, ClassifiedError, DiagnosticAuthSummary, DiagnosticExport,
    DiagnosticExportBuilder, DiagnosticExportBuilderInput, FetchAttempt, FetchError, FetchOutcome,
    FetchResult, redact_diagnostic_text,
};
use ariadusage_core::providers::{SourceMode, find_by_id_str};
use ariadusage_engine::config_store::ConfigStore;
use ariadusage_protocol::metric::SourceKind;
use ariadusage_protocol::usage::{ProviderErrorCategory, ProviderErrorKind};
use ariadusage_protocol::{Confidence, DetailRow, DetailSection, ProviderId};
use assert_cmd::cargo::cargo_bin;
use jiff::Timestamp;

// upstream: CLIDiagnoseCommandTests.swift:7 diagnose help describes generic JSON export
#[test]
fn diagnose_help_describes_generic_json_export() {
    let help = diagnose_help("0.0.0");

    assert!(help.contains("ariadusage diagnose --provider <name|all> --format json"));
    assert!(help.contains("ariadusage diagnose --provider all --format json"));
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
        .env_clear()
        .env("HOME", root)
        .env("CFFIXED_USER_HOME", root)
        .env("CODEX_HOME", root.join(".codex"))
        .env("GEMINI_CLI_HOME", &gemini_home)
        .env("XDG_RUNTIME_DIR", &runtime_dir)
        .env("ARIADUSAGE_CONFIG", &config_path)
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
        .env_clear()
        .env("HOME", root)
        .env("CFFIXED_USER_HOME", root)
        .env("CODEX_HOME", root.join(".codex"))
        .env("GEMINI_CLI_HOME", &gemini_home)
        .env("XDG_RUNTIME_DIR", &runtime_dir)
        .env("ARIADUSAGE_CONFIG", &config_path)
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

// Rust-added: verifies CLI diagnose and redaction filter strip token, cookie, email, and account id
#[test]
fn diagnose_output_never_contains_secret_token_cookie_email_or_account_id() {
    let temp = tempfile::tempdir().expect("tempdir");
    let root = temp.path();
    let config_path = root.join("config.json");
    let claude_id = ProviderId::new("claude").expect("claude id");
    let mut claude_cfg = ProviderConfig::new(claude_id.clone());
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
    let fake_account = "acct_invented_account_123456";

    let now = Timestamp::from_second(1_700_000_000).expect("timestamp");
    let descriptor = find_by_id_str("claude").expect("claude descriptor");
    let snapshot = UsageSnapshot::new(
        None,
        None,
        None,
        None,
        None,
        vec![DetailSection {
            title: Some(format!("Account {fake_email}")),
            rows: vec![
                DetailRow::new(
                    None::<&str>,
                    format!("Token {fake_token}"),
                    format!("{}: {fake_cookie}", "Cookie"),
                    Some(format!("{fake_account} Bearer {fake_token}")),
                    None,
                    None,
                )
                .expect("detail row"),
            ],
            chart: None,
        }],
        None,
        None,
        now,
        Some(ProviderIdentitySnapshot::new(
            Some(claude_id.clone()),
            Some(fake_email.to_owned()),
            Some(fake_account.to_owned()),
            Some("oauth".to_owned()),
            Some(fake_account.to_owned()),
        )),
        Confidence::Exact,
    )
    .expect("snapshot");

    let success_outcome = FetchOutcome {
        result: Ok(FetchResult::new(
            snapshot,
            "oauth",
            "claude.oauth",
            SourceKind::Oauth,
        )),
        attempts: vec![FetchAttempt {
            strategy_id: "claude.oauth".to_owned(),
            kind: SourceKind::Oauth,
            was_available: true,
            failure: None,
        }],
        failure_summary: None,
    };
    let success_export = DiagnosticExportBuilder::build(DiagnosticExportBuilderInput {
        provider: &claude_id,
        descriptor,
        outcome: &success_outcome,
        source_mode: SourceMode::Oauth,
        api_region: None,
        auth: DiagnosticAuthSummary::new(true, vec!["oauth".to_owned()]),
        app_version: None,
        timestamp: Some(now),
    });

    let make_classified = || {
        ClassifiedError::new(
            ProviderErrorKind::AuthenticationExpired,
            format!(
                "auth failed for {fake_email} ({fake_account}) with {fake_token} and {}: {fake_cookie}",
                "Cookie"
            ),
        )
    };
    let failure_outcome = FetchOutcome {
        result: Err(FetchError::Classified(make_classified())),
        attempts: vec![FetchAttempt {
            strategy_id: "claude.oauth".to_owned(),
            kind: SourceKind::Oauth,
            was_available: true,
            failure: Some(AttemptFailure {
                kind: ProviderErrorKind::AuthenticationExpired,
                category: ProviderErrorCategory::Auth,
            }),
        }],
        failure_summary: None,
    };
    let failure_export = DiagnosticExportBuilder::build(DiagnosticExportBuilderInput {
        provider: &claude_id,
        descriptor,
        outcome: &failure_outcome,
        source_mode: SourceMode::Oauth,
        api_region: None,
        auth: DiagnosticAuthSummary::new(true, vec!["oauth".to_owned()]),
        app_version: None,
        timestamp: Some(now),
    });

    for (idx, exp) in [success_export, failure_export].iter().enumerate() {
        let encoded = serde_json::to_string_pretty(exp).expect("encode export");
        let redacted = redact_diagnostic_text(&encoded);
        let out_file = root.join(format!("diag-{idx}.json"));
        write_diagnostic_export(&redacted, out_file.to_str().expect("utf8 path"))
            .expect("write diagnostic export");
        let written = std::fs::read_to_string(&out_file).expect("read diagnostic export");
        assert!(!written.contains(fake_token));
        assert!(!written.contains(fake_cookie));
        assert!(!written.contains(fake_email));
        assert!(!written.contains(fake_account));
    }

    let result = Command::new(&bin)
        .args([
            "diagnose",
            "--provider",
            "claude",
            "--format",
            "json",
            "--pretty",
        ])
        .env_clear()
        .env("HOME", root)
        .env("CFFIXED_USER_HOME", root)
        .env("XDG_RUNTIME_DIR", &runtime_dir)
        .env("ARIADUSAGE_CONFIG", &config_path)
        .env("ARIADUSAGE_DISABLE_KEYRING", "1")
        .env("CLAUDE_CODE_OAUTH_TOKEN", fake_token)
        .output()
        .expect("run claude diagnose");

    assert_eq!(result.status.code(), Some(0));
    let stdout = String::from_utf8(result.stdout).expect("utf8 stdout");
    assert!(!stdout.contains(fake_token));
    assert!(!stdout.contains(fake_cookie));
    assert!(!stdout.contains(fake_email));
    assert!(!stdout.contains(fake_account));
}

// Rust-added: verifies Antigravity CLI diagnose resolves OAuth credentials under user HOME rather than config directory
#[test]
fn antigravity_diagnose_finds_credentials_file_under_temporary_home() {
    let temp = tempfile::tempdir().expect("tempdir");
    let root = temp.path();

    let default_config_path = root.join(".config").join("ariadusage").join("config.json");
    let mut antigravity_cfg =
        ProviderConfig::new(ProviderId::new("antigravity").expect("antigravity id"));
    antigravity_cfg.enabled = Some(true);
    antigravity_cfg.source = Some(SourceMode::Oauth);
    ConfigStore::new(default_config_path)
        .save(&Config::new(
            Config::CURRENT_VERSION,
            vec![ProviderEntry::Typed(antigravity_cfg)],
        ))
        .expect("save config under default HOME/.config/ariadusage/config.json");

    let creds_dir = root.join(".ariadusage").join("antigravity");
    std::fs::create_dir_all(&creds_dir).expect("create .ariadusage/antigravity");
    std::fs::write(creds_dir.join("oauth_creds.json"), b"{not-json}")
        .expect("write oauth_creds.json");

    let runtime_dir = root.join("runtime");
    std::fs::create_dir_all(&runtime_dir).expect("create runtime dir");

    let mut cmd = Command::new(cargo_bin("ariadusage"));
    cmd.args([
        "diagnose",
        "--provider",
        "antigravity",
        "--format",
        "json",
        "--pretty",
    ])
    .env_clear()
    .env("HOME", root)
    .env("CFFIXED_USER_HOME", root)
    .env("XDG_RUNTIME_DIR", &runtime_dir)
    .env("ARIADUSAGE_DISABLE_KEYRING", "1");
    #[cfg(windows)]
    cmd.env("USERPROFILE", root);

    let result = cmd.output().expect("run diagnose with temporary HOME");
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
        Some("parse")
    );
}
