// Ported from CodexBar Sources/CodexBarCLI/CLIEntry.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBarCore/Logging/CodexBarLog.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBarCore/Logging/JSONStderrLogHandler.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBarCore/Logging/LogRedactor.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

use std::collections::BTreeMap;

use ariadusage_cli::{
    CliLogDestination, CliLogLevel, CliLoggingConfiguration, JsonLogLine, LogCategories,
    LogRedactor, ProviderLogging, bootstrap_cli_logging, captured_log_lines,
    clear_captured_log_lines, current_cli_log_level, current_cli_logging_configuration, logger,
    resolved_log_level, set_cli_log_level,
};
use ariadusage_core::config::{Config, ProviderConfig, ProviderEntry};
use ariadusage_engine::config_store::ConfigStore;
use ariadusage_protocol::ProviderId;

// Rust-added: smoke test for LogRedactor and ProviderLogging redacting emails, tokens, cookies, and auth headers
#[test]
fn log_redactor_redacts_emails_tokens_cookies_and_headers() {
    let raw = "Failed for user@example.com with sk-cp-abcDEF_123456";
    let redacted = LogRedactor::redact(raw);
    assert!(!redacted.contains("user@example.com"));
    assert!(!redacted.contains("sk-cp-abcDEF_123456"));
    assert!(redacted.contains("<redacted-email>"));
    assert!(redacted.contains("<redacted-minimax-token>"));

    assert_eq!(
        LogRedactor::redact("Token sk-api-xyz987_654321. Retry later."),
        "Token <redacted-minimax-token>. Retry later."
    );
    assert_eq!(
        LogRedactor::redact("Token (sk-api-xyz987_654321) [sk-cp-abcDEF_123456]; retry"),
        "Token (<redacted-minimax-token>) [<redacted-minimax-token>]; retry"
    );
    assert_eq!(
        LogRedactor::redact("Bearer token-value-123"),
        "Bearer <redacted>"
    );

    let header_raw = format!(
        "Authorization: Bearer {} and {}: session=abc; HERTZ-SESSION=xyz",
        concat!("sk-cp-", "abcDEF_123456"),
        "Cookie"
    );
    let header_redacted = ProviderLogging::redact(&header_raw);
    assert!(!header_redacted.contains("abcDEF_123456"));
    assert!(!header_redacted.contains("session=abc"));
    assert!(!header_redacted.contains("HERTZ-SESSION=xyz"));
    assert!(header_redacted.contains("Authorization: <redacted>"));

    // Additional AriadUsage secrets, cookies, OAuth tokens, and JWTs
    let fake_ant = concat!("sk-ant-", "oat01-secretanttoken999");
    let fake_sk = concat!("sk-", "proj_secretkey1234567890");
    let fake_ya = concat!("ya29.", "a0AfH6SM-secretgoogleoauth");
    let fake_refresh = concat!("1//", "0gsecretrefreshvalue123");
    let fake_ghp = concat!("ghp_", "secretgithubtoken123456");
    let fake_jwt = concat!(
        "eyJhbGciOiJIUzI1NiJ9",
        ".",
        "eyJzdWIiOiIxMjM0NTYifQ",
        ".",
        "SflKxwRJSMeKKF2QT4fwpMeJf36POk6yJV_adQssw5c"
    );
    let fake_cookie = concat!("sessionKey=", "secret-cookie-val");
    let combined = format!(
        "ant={fake_ant} sk={fake_sk} ya={fake_ya} ref={fake_refresh} ghp={fake_ghp} jwt={fake_jwt} {fake_cookie}"
    );
    let scrubbed = LogRedactor::redact(&combined);
    for secret in [
        fake_ant,
        fake_sk,
        fake_ya,
        fake_refresh,
        fake_ghp,
        fake_jwt,
        "secret-cookie-val",
    ] {
        assert!(
            !scrubbed.contains(secret),
            "expected '{secret}' to be redacted in: {scrubbed}"
        );
    }
    assert_eq!(LogRedactor::redact(""), "");
    assert_eq!(
        LogRedactor::redact("Normal log without secrets"),
        "Normal log without secrets"
    );
}

// Rust-added: smoke test for CliLogLevel resolution, CliLoggingConfiguration routing, and CliLogger redaction
#[test]
fn cli_logging_configuration_and_logger_redact_messages_and_metadata() {
    assert_eq!(resolved_log_level(false, None), CliLogLevel::Error);
    assert_eq!(resolved_log_level(true, None), CliLogLevel::Debug);
    assert_eq!(resolved_log_level(false, Some("info")), CliLogLevel::Info);
    assert_eq!(
        resolved_log_level(true, Some("critical")),
        CliLogLevel::Critical
    );
    assert_eq!(
        resolved_log_level(true, Some("invalid")),
        CliLogLevel::Debug
    );

    let diag_cfg =
        CliLoggingConfiguration::for_command(true, true, false, Some("trace"), &["diagnose"]);
    assert_eq!(diag_cfg.destination, CliLogDestination::Discard);
    assert_eq!(diag_cfg.level, CliLogLevel::Trace);
    assert!(diag_cfg.json);

    let cost_cfg = CliLoggingConfiguration::for_command(true, false, true, None, &["cost"]);
    assert_eq!(cost_cfg.destination, CliLogDestination::Stderr);
    assert_eq!(cost_cfg.level, CliLogLevel::Debug);
    assert!(cost_cfg.json);

    bootstrap_cli_logging(CliLoggingConfiguration::new(
        CliLogDestination::Discard,
        CliLogLevel::Debug,
        true,
    ));
    assert_eq!(current_cli_log_level(), CliLogLevel::Debug);
    clear_captured_log_lines();

    let fake_token = concat!("sk-ant-", "oat01-supersecretlogger123");
    let log = logger(LogCategories::TOKEN_COST);
    log.trace("Filtered trace line", None);
    log.debug(
        &format!("Scanning for owner@example.com with {fake_token}"),
        Some(BTreeMap::from([("auth", format!("Bearer {fake_token}"))])),
    );

    let captured = captured_log_lines();
    assert_eq!(captured.len(), 1);
    assert_eq!(captured[0].level, "debug");
    assert_eq!(captured[0].label, LogCategories::SUBSYSTEM);
    assert!(!captured[0].message.contains("owner@example.com"));
    assert!(!captured[0].message.contains(fake_token));
    assert!(captured[0].message.contains("<redacted-email>"));
    let meta = captured[0].metadata.as_ref().expect("metadata");
    assert_eq!(
        meta.get("category").map(String::as_str),
        Some(LogCategories::TOKEN_COST)
    );
    assert_eq!(
        meta.get("auth").map(String::as_str),
        Some("Bearer <redacted>")
    );

    set_cli_log_level(CliLogLevel::Warning);
    assert_eq!(
        current_cli_logging_configuration().map(|c| c.level),
        Some(CliLogLevel::Warning)
    );
}

fn parse_stderr_json_logs(stderr: &[u8]) -> Vec<JsonLogLine> {
    let text = String::from_utf8_lossy(stderr);
    text.lines()
        .filter(|line| line.trim_start().starts_with('{'))
        .filter_map(|line| serde_json::from_str::<JsonLogLine>(line).ok())
        .collect()
}

// Rust-added: end-to-end binary smoke test verifying -v/--verbose, --log-level, and --json-output across all 7 CLI commands
#[test]
fn cli_logging_flags_configure_stderr_json_logs_across_all_commands() {
    let temp = tempfile::tempdir().expect("tempdir");
    let root = temp.path();
    let home_dir = root.join("home");
    let runtime_dir = root.join("runtime");
    let xdg_cache = root.join("cache");
    std::fs::create_dir_all(&home_dir).expect("create home");
    std::fs::create_dir_all(&runtime_dir).expect("create runtime");
    std::fs::create_dir_all(&xdg_cache).expect("create cache");

    let config_path = root.join("config.json");
    let mut codex_cfg = ProviderConfig::new(ProviderId::new("codex").expect("codex id"));
    codex_cfg.enabled = Some(true);
    ConfigStore::new(config_path.clone())
        .save(&Config::new(
            Config::CURRENT_VERSION,
            vec![ProviderEntry::Typed(codex_cfg)],
        ))
        .expect("save config");

    let run_cmd = |args: &[&str]| -> std::process::Output {
        assert_cmd::Command::new(env!("CARGO_BIN_EXE_ariadusage"))
            .env_clear()
            .env("HOME", &home_dir)
            .env("CFFIXED_USER_HOME", &home_dir)
            .env("CODEX_HOME", home_dir.join(".codex"))
            .env("GEMINI_CLI_HOME", home_dir.join(".gemini"))
            .env("XDG_CACHE_HOME", &xdg_cache)
            .env("XDG_RUNTIME_DIR", &runtime_dir)
            .env("ARIADUSAGE_CONFIG", &config_path)
            .env("ARIADUSAGE_DISABLE_KEYRING", "1")
            .args(args)
            .output()
            .expect("spawn ariadusage")
    };

    // 1. cost
    let cost_out = run_cmd(&[
        "cost",
        "--provider",
        "codex",
        "--json",
        "-v",
        "--log-level",
        "debug",
        "--json-output",
    ]);
    assert!(cost_out.status.success());
    let cost_logs = parse_stderr_json_logs(&cost_out.stderr);
    assert!(
        cost_logs.iter().any(|l| {
            l.level == "debug"
                && l.metadata
                    .as_ref()
                    .and_then(|m| m.get("category"))
                    .map(String::as_str)
                    == Some(LogCategories::TOKEN_COST)
        }),
        "expected token-cost JSON log on stderr, got: {}",
        String::from_utf8_lossy(&cost_out.stderr)
    );

    // 2. config
    let config_out = run_cmd(&[
        "config",
        "validate",
        "--json",
        "-v",
        "--log-level",
        "debug",
        "--json-output",
    ]);
    assert!(config_out.status.success());
    let config_logs = parse_stderr_json_logs(&config_out.stderr);
    assert!(
        config_logs.iter().any(|l| {
            l.level == "debug"
                && l.metadata
                    .as_ref()
                    .and_then(|m| m.get("category"))
                    .map(String::as_str)
                    == Some(LogCategories::CONFIG_STORE)
        }),
        "expected config-store JSON log on stderr, got: {}",
        String::from_utf8_lossy(&config_out.stderr)
    );

    // 3. hooks
    let hooks_out = run_cmd(&[
        "hooks",
        "list",
        "--json",
        "-v",
        "--log-level",
        "debug",
        "--json-output",
    ]);
    assert!(hooks_out.status.success());
    let hooks_logs = parse_stderr_json_logs(&hooks_out.stderr);
    assert!(
        hooks_logs.iter().any(|l| {
            l.level == "debug"
                && l.metadata
                    .as_ref()
                    .and_then(|m| m.get("category"))
                    .map(String::as_str)
                    == Some(LogCategories::HOOKS)
        }),
        "expected hooks JSON log on stderr, got: {}",
        String::from_utf8_lossy(&hooks_out.stderr)
    );

    // 4. cache clear
    let cache_out = run_cmd(&[
        "cache",
        "clear",
        "--cost",
        "--json",
        "-v",
        "--log-level",
        "debug",
        "--json-output",
    ]);
    assert!(cache_out.status.success());
    let cache_logs = parse_stderr_json_logs(&cache_out.stderr);
    assert!(
        cache_logs.iter().any(|l| {
            l.level == "debug"
                && l.metadata
                    .as_ref()
                    .and_then(|m| m.get("category"))
                    .map(String::as_str)
                    == Some(LogCategories::CACHE)
        }),
        "expected cache JSON log on stderr, got: {}",
        String::from_utf8_lossy(&cache_out.stderr)
    );

    // 5. cookie refresh
    let cookie_out = run_cmd(&[
        "cookie",
        "refresh",
        "--provider",
        "claude",
        "--json",
        "-v",
        "--log-level",
        "debug",
        "--json-output",
    ]);
    let cookie_logs = parse_stderr_json_logs(&cookie_out.stderr);
    assert!(
        cookie_logs.iter().any(|l| {
            l.level == "debug"
                && l.metadata
                    .as_ref()
                    .and_then(|m| m.get("category"))
                    .map(String::as_str)
                    == Some(LogCategories::COOKIE_REFRESH)
        }),
        "expected cookie-refresh JSON log on stderr, got: {}",
        String::from_utf8_lossy(&cookie_out.stderr)
    );

    // 6. usage
    let usage_out = run_cmd(&[
        "usage",
        "--provider",
        "antigravity",
        "-v",
        "--log-level",
        "debug",
        "--json-output",
    ]);
    let usage_logs = parse_stderr_json_logs(&usage_out.stderr);
    assert!(
        usage_logs.iter().any(|l| {
            l.level == "debug"
                && l.metadata
                    .as_ref()
                    .and_then(|m| m.get("category"))
                    .map(String::as_str)
                    == Some("provider.antigravity")
        }),
        "expected provider.antigravity JSON log on stderr, got: {}",
        String::from_utf8_lossy(&usage_out.stderr)
    );

    // 7. diagnose routes logging to .discard so stderr remains empty
    let diag_out = run_cmd(&[
        "diagnose",
        "--provider",
        "codex",
        "--format",
        "json",
        "-v",
        "--log-level",
        "trace",
        "--json-output",
    ]);
    assert!(
        diag_out.status.success(),
        "diagnose stderr: {}",
        String::from_utf8_lossy(&diag_out.stderr)
    );
    assert!(
        diag_out.stderr.is_empty(),
        "expected diagnose to discard stderr logs, got: {}",
        String::from_utf8_lossy(&diag_out.stderr)
    );
}
