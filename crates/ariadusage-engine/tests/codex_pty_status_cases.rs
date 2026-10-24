// Ported from CodexBar Tests/CodexBarTests/StatusProbeTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/TTYCommandRunnerTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/CodexCLIDiscoveryTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/CodexUserFacingErrorTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

#![cfg(unix)]

use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use ariadusage_core::gates::launch::LaunchGate;
use ariadusage_core::pipeline::FetchInteraction;
use ariadusage_engine::brokers::call::BrokerCall;
use ariadusage_engine::brokers::process::{AbsolutePath, ProcessRegistry};
use ariadusage_engine::providers::codex::{
    CodexCLILaunchGate, CodexCLISession, CodexCLIUsageFetcher, CodexStatusProbe,
    CodexStatusProbeError, CodexStatusProbeIsolation, CodexUIErrorMapper, CodexUserFacingErrors,
};
use ariadusage_protocol::ProviderId;
use jiff::tz::TimeZone;
use jiff::{Timestamp, civil};
use serde_json::json;
use tempfile::TempDir;
use tokio_util::sync::CancellationToken;

fn system_date(
    year: i16,
    month: i8,
    day: i8,
    hour: i8,
    minute: i8,
) -> Result<Timestamp, jiff::Error> {
    let tz = TimeZone::system();
    let dt = civil::Date::new(year, month, day)?.at(hour, minute, 0, 0);
    Ok(dt.to_zoned(tz)?.timestamp())
}

// ============================================================================
// StatusProbeTests.swift (Codex cases)
// ============================================================================

// upstream: StatusProbeTests.swift:7 parse codex status
#[test]
fn parse_codex_status() {
    let sample = "\
Model: gpt
Credits: 980 credits
5h limit: [#####] 75% left
Weekly limit: [##] 25% left";
    let now = Timestamp::from_second(1_776_000_000).unwrap();
    let snap = CodexStatusProbe::parse(sample, now).unwrap();
    // upstream-assert: StatusProbeTests.swift:16 #expect
    assert_eq!(snap.credits, Some(980.0));
    // upstream-assert: StatusProbeTests.swift:17 #expect
    assert_eq!(snap.five_hour_percent_left, Some(75));
    // upstream-assert: StatusProbeTests.swift:18 #expect
    assert_eq!(snap.weekly_percent_left, Some(25));
}

// upstream: StatusProbeTests.swift:21 parse codex status with ansi and resets
#[test]
fn parse_codex_status_with_ansi_and_resets() {
    let now_res = system_date(2026, 11, 26, 8, 0);
    // upstream-assert: StatusProbeTests.swift:23 #require
    assert!(now_res.is_ok());
    let now = now_res.unwrap();
    let sample = "\
\u{001B}[38;5;245mCredits:\u{001B}[0m 557 credits
5h limit: [█████     ] 50% left (resets 09:01)
Weekly limit: [███████   ] 85% left (resets 04:01 on 27 Nov)";
    let snap = CodexStatusProbe::parse(sample, now).unwrap();
    let expected_five_hour = system_date(2026, 11, 26, 9, 1).unwrap();
    let expected_weekly = system_date(2026, 11, 27, 4, 1).unwrap();

    // upstream-assert: StatusProbeTests.swift:37 #expect
    assert_eq!(snap.credits, Some(557.0));
    // upstream-assert: StatusProbeTests.swift:38 #expect
    assert_eq!(snap.five_hour_percent_left, Some(50));
    // upstream-assert: StatusProbeTests.swift:39 #expect
    assert_eq!(snap.weekly_percent_left, Some(85));
    // upstream-assert: StatusProbeTests.swift:40 #expect
    assert_eq!(snap.five_hour_resets_at, Some(expected_five_hour));
    // upstream-assert: StatusProbeTests.swift:47 #expect
    assert_eq!(snap.weekly_resets_at, Some(expected_weekly));
}

// upstream: StatusProbeTests.swift:56 parse codex status with weekly only line
#[test]
fn parse_codex_status_with_weekly_only_line() {
    let sample = "\
Model: gpt
Credits: 980 credits
Weekly limit: [##] 25% left";
    let now = Timestamp::from_second(1_776_000_000).unwrap();
    let snap = CodexStatusProbe::parse(sample, now).unwrap();
    // upstream-assert: StatusProbeTests.swift:64 #expect
    assert_eq!(snap.credits, Some(980.0));
    // upstream-assert: StatusProbeTests.swift:65 #expect
    assert_eq!(snap.five_hour_percent_left, None);
    // upstream-assert: StatusProbeTests.swift:66 #expect
    assert_eq!(snap.weekly_percent_left, Some(25));
}

// upstream: StatusProbeTests.swift:69 parse codex monthly credit limit
#[test]
fn parse_codex_monthly_credit_limit() {
    let now_res = system_date(2026, 6, 23, 12, 0);
    // upstream-assert: StatusProbeTests.swift:71 #require
    assert!(now_res.is_ok());
    let now = now_res.unwrap();
    let sample = "\
Model: codex-status-fixture
Monthly credit limit: [██████████████████░░] 92% left (resets 08:00 on 1 Jul)
                    7,761 of 100,000 credits used";
    let snap = CodexStatusProbe::parse(sample, now).unwrap();
    let limit = snap.codex_credit_limit.as_ref().unwrap();
    let expected_resets_at = system_date(2026, 7, 1, 8, 0).unwrap();

    // upstream-assert: StatusProbeTests.swift:87 #expect
    assert_eq!(limit.limit, 100_000.0);
    // upstream-assert: StatusProbeTests.swift:88 #expect
    assert_eq!(limit.used, 7_761.0);
    // upstream-assert: StatusProbeTests.swift:89 #expect
    assert_eq!(limit.remaining, 92_239.0);
    // upstream-assert: StatusProbeTests.swift:90 #expect
    assert_eq!(limit.remaining_percent, 92.0);
    // upstream-assert: StatusProbeTests.swift:91 #expect
    assert_eq!(limit.resets_at, Some(expected_resets_at));
}

// upstream: StatusProbeTests.swift:836 live codex status
#[tokio::test]
async fn live_codex_status() {
    let temp = TempDir::new().unwrap();
    let codex_home = temp.path().join(".codex");
    fs::create_dir_all(&codex_home).unwrap();
    fs::write(
        codex_home.join("test-pty-status.json"),
        serde_json::to_vec(&json!({
            "status_output": "\u{001B}[38;5;245mCredits:\u{001B}[0m 557 credits\r\n5h limit: [█████     ] 50% left (resets 09:01)\r\nWeekly limit: [███████   ] 85% left (resets 04:01 on 27 Nov)\r\n"
        }))
        .unwrap(),
    )
    .unwrap();

    let bin = AbsolutePath::new(env!("CARGO_BIN_EXE_ariadusage-test-child")).unwrap();
    let mut env = BTreeMap::new();
    env.insert(
        "HOME".to_owned(),
        temp.path().to_string_lossy().into_owned(),
    );
    env.insert(
        "CODEX_HOME".to_owned(),
        codex_home.to_string_lossy().into_owned(),
    );
    env.insert(
        "TMPDIR".to_owned(),
        temp.path().to_string_lossy().into_owned(),
    );

    let now = system_date(2026, 11, 26, 8, 0).unwrap();
    let probe = CodexStatusProbe::new()
        .with_environment(env)
        .with_home_directory(Some(temp.path().to_path_buf()))
        .with_state_directory(temp.path().join("state"))
        .with_executable(bin)
        .with_timeout(Duration::from_secs(5));

    let snap = probe.fetch(now).await.unwrap();
    assert_eq!(snap.five_hour_percent_left, Some(50));
    assert_eq!(snap.weekly_percent_left, Some(85));
    assert_eq!(snap.credits, Some(557.0));
}

// ============================================================================
// TTYCommandRunnerTests.swift (Codex status cases)
// ============================================================================

// upstream: TTYCommandRunnerTests.swift:225 codex status probe uses non persistent thread storage
#[test]
fn codex_status_probe_uses_non_persistent_thread_storage() {
    let state_home = Path::new("/tmp/codexbar status \"state\"");
    let args = CodexStatusProbeIsolation::codex_arguments(state_home);

    // upstream-assert: TTYCommandRunnerTests.swift:230 #expect
    assert_eq!(&args[..4], ["-s", "read-only", "-a", "never"]);
    // upstream-assert: TTYCommandRunnerTests.swift:231 #expect
    assert!(args.iter().any(|a| a == "history.persistence=\"none\""));
    // upstream-assert: TTYCommandRunnerTests.swift:232 #expect
    assert!(
        args.iter()
            .any(|a| a == "experimental_thread_store={type=\"in_memory\",id=\"codexbar-status\"}")
    );
    // upstream-assert: TTYCommandRunnerTests.swift:233 #expect
    assert!(
        args.iter()
            .any(|a| a == "sqlite_home=\"/tmp/codexbar status \\\"state\\\"\"")
    );
}

// upstream: TTYCommandRunnerTests.swift:236 codex status probe avoids root working directory when home exists
#[test]
fn codex_status_probe_avoids_root_working_directory_when_home_exists() {
    let home = "/Users/tester";
    let env = BTreeMap::from([("HOME".to_owned(), home.to_owned())]);
    let working_directory = CodexStatusProbeIsolation::working_directory(&env);
    // upstream-assert: TTYCommandRunnerTests.swift:240 #expect
    assert_eq!(working_directory.as_deref(), Some(Path::new(home)));
}

// ============================================================================
// CodexCLIDiscoveryTests.swift (:7 supplied environment rejection)
// ============================================================================

// upstream: CodexCLIDiscoveryTests.swift:7 supplied environment rejection cannot fall back to the process environment
#[tokio::test]
async fn supplied_environment_rejection_cannot_fall_back_to_the_process_environment() {
    let root = TempDir::new().unwrap();
    let package = root.path().join("node_modules/@openai/codex");
    let wrapper = package.join("bin/codex.js");
    let payload = package.join("vendor/aarch64-apple-darwin/bin/codex");
    for directory in [wrapper.parent().unwrap(), payload.parent().unwrap()] {
        fs::create_dir_all(directory).unwrap();
    }
    let marker = root.path().join("launched");
    let script = "\
#!/bin/sh
# path.join(vendorRoot, targetTriple, \"bin\")
touch \"$CODEXBAR_TEST_LAUNCH_MARKER\"
printf 'Credits: 1\\n'
";
    fs::write(&wrapper, script).unwrap();
    fs::set_permissions(&wrapper, fs::Permissions::from_mode(0o755)).unwrap();
    fs::write(&payload, "#!/bin/sh\nexit 0\n").unwrap();
    fs::set_permissions(&payload, fs::Permissions::from_mode(0o755)).unwrap();

    let environment = BTreeMap::from([
        (
            "PATH".to_owned(),
            root.path().to_string_lossy().into_owned(),
        ),
        (
            "HOME".to_owned(),
            root.path().to_string_lossy().into_owned(),
        ),
        (
            "TMPDIR".to_owned(),
            root.path().to_string_lossy().into_owned(),
        ),
        (
            "NODE_OPTIONS".to_owned(),
            "--require=/fixture/forbidden-preload.cjs".to_owned(),
        ),
        (
            "CODEXBAR_TEST_LAUNCH_MARKER".to_owned(),
            marker.to_string_lossy().into_owned(),
        ),
    ]);

    let wrapper_for_resolver = wrapper.clone();
    let resolver = move |supplied: &BTreeMap<String, String>| -> Option<PathBuf> {
        let disallowed = supplied
            .get("NODE_OPTIONS")
            .is_some_and(|opts| opts.contains("--require") || opts.contains("--import"));
        if disallowed {
            None
        } else {
            Some(wrapper_for_resolver.clone())
        }
    };

    // upstream-assert: CodexCLIDiscoveryTests.swift:44 #expect
    assert_eq!(resolver(&BTreeMap::new()), Some(wrapper.clone()));
    // upstream-assert: CodexCLIDiscoveryTests.swift:45 #expect
    assert_eq!(resolver(&environment), None);

    for entrypoint in ["pty", "status"] {
        let probe = CodexStatusProbe::new()
            .with_environment(environment.clone())
            .with_home_directory(Some(root.path().to_path_buf()))
            .with_state_directory(root.path().join("state"))
            .with_timeout(Duration::from_secs(5))
            .with_keep_cli_sessions_alive(entrypoint == "pty");
        let err = probe
            .fetch(Timestamp::from_second(1_776_000_000).unwrap())
            .await
            .unwrap_err();
        assert_eq!(err, CodexStatusProbeError::CodexNotInstalled);
        // upstream-assert: CodexCLIDiscoveryTests.swift:64 #expect
        assert!(!marker.exists());
    }
}

// ============================================================================
// CodexUserFacingErrorTests.swift
// ============================================================================

// upstream: CodexUserFacingErrorTests.swift:8 missing codex CLI guidance is not collapsed to not running
#[test]
fn missing_codex_cli_guidance_is_not_collapsed_to_not_running() {
    let raw = "Codex not running. Try running a Codex command first. \
(Codex CLI not found. Install with `npm i -g @openai/codex`.)";
    // upstream-assert: CodexUserFacingErrorTests.swift:14 #expect
    assert_eq!(
        CodexUIErrorMapper::user_facing_message(Some(raw)),
        Some(CodexStatusProbeError::CodexNotInstalled.to_string())
    );
}

// upstream: CodexUserFacingErrorTests.swift:17 logged out codex CLI guidance is not collapsed to temporary outage
#[test]
fn logged_out_codex_cli_guidance_is_not_collapsed_to_temporary_outage() {
    let raw = "Codex connection failed: codex account authentication required to read rate limits";
    // upstream-assert: CodexUserFacingErrorTests.swift:23 #expect
    assert_eq!(
        CodexUIErrorMapper::user_facing_message(Some(raw)).as_deref(),
        Some("Codex CLI is not signed in. Run `codex login --device-auth`, then refresh.")
    );
}

// upstream: CodexUserFacingErrorTests.swift:28 cached logged out codex CLI failure preserves cached suffix
#[test]
fn cached_logged_out_codex_cli_failure_preserves_cached_suffix() {
    let raw = "Last Codex credits refresh failed: Codex connection failed: \
codex account authentication required to read rate limits. Cached values from 2m ago.";
    // upstream-assert: CodexUserFacingErrorTests.swift:35 #expect
    assert_eq!(
        CodexUIErrorMapper::user_facing_message(Some(raw)).as_deref(),
        Some(
            "Codex CLI is not signed in. Run `codex login --device-auth`, then refresh. \
Cached values from 2m ago."
        )
    );
}

// upstream: CodexUserFacingErrorTests.swift:41 expired codex auth is sanitized
#[test]
fn expired_codex_auth_is_sanitized() {
    let raw = "Codex connection failed: failed to fetch codex rate limits: GET https://chatgpt.com/backend-api/wham/usage \
failed: 401 Unauthorized; content-type=text/plain; body={\"error\":{\"message\":\"Provided authentication \
token is expired. Please try signing in again.\",\"code\":\"token_expired\"}}";
    // upstream-assert: CodexUserFacingErrorTests.swift:50 #expect
    assert_eq!(
        CodexUIErrorMapper::user_facing_message(Some(raw)).as_deref(),
        Some("Codex session expired. Sign in again.")
    );
}

// upstream: CodexUserFacingErrorTests.swift:53 transport codex error is sanitized
#[test]
fn transport_codex_error_is_sanitized() {
    let raw = "Codex connection failed: failed to fetch codex rate limits: \
GET https://chatgpt.com/backend-api/wham/usage failed: 500";
    // upstream-assert: CodexUserFacingErrorTests.swift:60 #expect
    assert_eq!(
        CodexUIErrorMapper::user_facing_message(Some(raw)).as_deref(),
        Some("Codex usage is temporarily unavailable. Try refreshing.")
    );
}

// upstream: CodexUserFacingErrorTests.swift:63 decode mismatch codex error is sanitized
#[test]
fn decode_mismatch_codex_error_is_sanitized() {
    let raw = "Codex connection failed: failed to fetch codex rate limits: \
Decode error for https://chatgpt.com/backend-api/wham/usage: \
unknown variant `prolite`, expected one of `guest`, `free`, `go`, `plus`, `pro`";
    // upstream-assert: CodexUserFacingErrorTests.swift:71 #expect
    assert_eq!(
        CodexUIErrorMapper::user_facing_message(Some(raw)).as_deref(),
        Some("Codex usage is temporarily unavailable. Try refreshing.")
    );
}

// upstream: CodexUserFacingErrorTests.swift:74 cached credits failure preserves cached suffix while sanitizing body
#[test]
fn cached_credits_failure_preserves_cached_suffix_while_sanitizing_body() {
    let raw = "Last Codex credits refresh failed: Codex connection failed: failed to fetch codex rate limits: \
GET https://chatgpt.com/backend-api/wham/usage failed: 500; body={\"error\":{}} \
Cached values from 2m ago.";
    // upstream-assert: CodexUserFacingErrorTests.swift:82 #expect
    assert_eq!(
        CodexUIErrorMapper::user_facing_message(Some(raw)).as_deref(),
        Some("Codex usage is temporarily unavailable. Try refreshing. Cached values from 2m ago.")
    );
}

// upstream: CodexUserFacingErrorTests.swift:101 cached missing codex CLI failure preserves cached suffix
#[test]
fn cached_missing_codex_cli_failure_preserves_cached_suffix() {
    let raw = "Last Codex credits refresh failed: Codex CLI not found. \
Install with `npm i -g @openai/codex`. Cached values from 2m ago.";
    let expected = format!(
        "{} Cached values from 2m ago.",
        CodexStatusProbeError::CodexNotInstalled
    );
    // upstream-assert: CodexUserFacingErrorTests.swift:108 #expect
    assert_eq!(
        CodexUIErrorMapper::user_facing_message(Some(raw)),
        Some(expected)
    );
}

// partial-upstream: CodexUserFacingErrorTests.swift:113 browser mismatch remains unchanged
#[test]
fn browser_mismatch_remains_unchanged() {
    let raw = "OpenAI cookies are for user-a@example.com, not user-b@example.com. \
Switch chatgpt.com account, then refresh OpenAI cookies.";
    // partial-upstream-assert: CodexUserFacingErrorTests.swift:120 #expect
    assert_eq!(
        CodexUIErrorMapper::user_facing_message(Some(raw)).as_deref(),
        Some(raw)
    );
}

// upstream: CodexUserFacingErrorTests.swift:126 frame load interrupted becomes retry guidance
#[test]
fn frame_load_interrupted_becomes_retry_guidance() {
    let raw = "Frame load interrupted";
    // upstream-assert: CodexUserFacingErrorTests.swift:131 #expect
    assert_eq!(
        CodexUIErrorMapper::user_facing_message(Some(raw)).as_deref(),
        Some("OpenAI web refresh was interrupted. Refresh OpenAI cookies and try again.")
    );
}

// upstream: CodexUserFacingErrorTests.swift:136 open A I web timeout becomes retry guidance
#[test]
fn open_ai_web_timeout_becomes_retry_guidance() {
    let raw = "The operation couldn\u{2019}t be completed. (NSURLErrorDomain error -1001.)";
    // upstream-assert: CodexUserFacingErrorTests.swift:141 #expect
    assert_eq!(
        CodexUIErrorMapper::user_facing_message(Some(raw)).as_deref(),
        Some("OpenAI web refresh timed out. Refresh OpenAI cookies and try again.")
    );
}

// upstream: CodexUserFacingErrorTests.swift:163 open A I web network error becomes connection guidance
#[test]
fn open_ai_web_network_error_becomes_connection_guidance() {
    let raw = "The operation couldn\u{2019}t be completed. (NSURLErrorDomain error -1004.)";
    let expected = [
        "OpenAI web refresh hit a network error.",
        "Check your connection, then refresh OpenAI cookies and try again.",
    ]
    .join(" ");
    // upstream-assert: CodexUserFacingErrorTests.swift:172 #expect
    assert_eq!(
        CodexUIErrorMapper::user_facing_message(Some(raw)),
        Some(expected)
    );
}

// upstream: CodexUserFacingErrorTests.swift:175 non codex providers keep raw errors
#[test]
fn non_codex_providers_keep_raw_errors() {
    let claude = ProviderId::new("claude").unwrap();
    let raw = "Claude probe failed with debug detail";
    // upstream-assert: CodexUserFacingErrorTests.swift:180 #expect
    assert_eq!(
        CodexUIErrorMapper::user_facing_error_for_provider(claude.as_str(), Some(raw)).as_deref(),
        Some(raw)
    );
}

// upstream: CodexUserFacingErrorTests.swift:198 providers pane codex model uses sanitized values
#[test]
fn providers_pane_codex_model_uses_sanitized_values() {
    let mapped = CodexUIErrorMapper::user_facing_errors(
        Some(
            "Codex connection failed: failed to fetch codex rate limits: \
GET https://chatgpt.com/backend-api/wham/usage failed: 500",
        ),
        Some(
            "Last Codex credits refresh failed: Codex connection failed: failed to fetch codex rate limits: \
GET https://chatgpt.com/backend-api/wham/usage failed: 500 \
Cached values from 1m ago.",
        ),
        Some("Frame load interrupted"),
        true,
    );

    // upstream-assert: CodexUserFacingErrorTests.swift:214 #expect
    assert_eq!(
        mapped.usage.as_deref(),
        Some("Codex usage is temporarily unavailable. Try refreshing.")
    );
    // upstream-assert: CodexUserFacingErrorTests.swift:215 #expect
    assert_eq!(
        mapped.dashboard.as_deref(),
        Some("OpenAI web refresh was interrupted. Refresh OpenAI cookies and try again.")
    );
    // upstream-assert: CodexUserFacingErrorTests.swift:218 #expect
    assert_eq!(
        mapped.credits_hint_copy_text(),
        Some("OpenAI web refresh was interrupted. Refresh OpenAI cookies and try again.")
    );
    // upstream-assert: CodexUserFacingErrorTests.swift:221 #expect
    assert_eq!(
        mapped.credits.as_deref(),
        Some("Codex usage is temporarily unavailable. Try refreshing. Cached values from 1m ago.")
    );
}

// upstream: CodexUserFacingErrorTests.swift:257 providers pane codex error display keeps raw full text for copy
#[test]
fn providers_pane_codex_error_display_keeps_raw_full_text_for_copy() {
    let raw = "Codex connection failed: failed to fetch codex rate limits: \
GET https://chatgpt.com/backend-api/wham/usage failed: 500; body={\"error\":{}}";
    let display = CodexUIErrorMapper::provider_error_display("codex", Some(raw)).unwrap();

    // upstream-assert: CodexUserFacingErrorTests.swift:269 #expect
    assert_eq!(
        display.preview,
        "Codex usage is temporarily unavailable. Try refreshing."
    );
    // upstream-assert: CodexUserFacingErrorTests.swift:270 #expect
    assert_eq!(display.full, raw);
}

// ============================================================================
// Additional PTY /status session, retry, launch-gate, and secret-redaction tests
// ============================================================================

#[tokio::test]
async fn codex_pty_warm_session_reuse_retry_on_parse_failed_update_prompt_and_secret_stripping() {
    let temp = TempDir::new().unwrap();
    let codex_home = temp.path().join(".codex");
    let state_dir = temp.path().join("state");
    fs::create_dir_all(&codex_home).unwrap();

    let status_file = codex_home.join("test-pty-status.json");
    let invocations_file = codex_home.join("test-pty-invocations.jsonl");
    let bin = AbsolutePath::new(env!("CARGO_BIN_EXE_ariadusage-test-child")).unwrap();

    let mut env = BTreeMap::new();
    env.insert(
        "HOME".to_owned(),
        temp.path().to_string_lossy().into_owned(),
    );
    env.insert(
        "CODEX_HOME".to_owned(),
        codex_home.to_string_lossy().into_owned(),
    );
    env.insert(
        "TMPDIR".to_owned(),
        temp.path().to_string_lossy().into_owned(),
    );
    // Secrets that must be stripped by ProcessEnv::from_allowlist
    env.insert(
        "ARIADUSAGE_SECRET_TOKEN".to_owned(),
        "top-secret-ariadusage".to_owned(),
    );
    env.insert(
        "OPENAI_API_KEY".to_owned(),
        "sk-openai-secret-key".to_owned(),
    );
    env.insert("CODEX_API_KEY".to_owned(), "sk-codex-secret-key".to_owned());

    let now = system_date(2026, 11, 26, 8, 0).unwrap();

    // 1. Warm session reuse + cursor query (\x1b[6n) + update prompt skip (\x1b[B) + secret env stripping
    fs::write(
        &status_file,
        serde_json::to_vec(&json!({
            "send_cursor_query": true,
            "send_update_prompt": true,
            "status_output": "Credits: 412 credits\r\n5h limit: [######] 60% left (resets 09:01)\r\nWeekly limit: [########] 80% left (resets 04:01 on 27 Nov)\r\n",
            "record_invocation_path": invocations_file.to_string_lossy()
        }))
        .unwrap(),
    )
    .unwrap();

    let session = CodexCLISession::new(
        Arc::new(ProcessRegistry::new()),
        Arc::new(LaunchGate::default()),
    )
    .with_state_directory(Ok(state_dir.clone()));
    let probe = CodexStatusProbe::new()
        .with_environment(env.clone())
        .with_home_directory(Some(temp.path().to_path_buf()))
        .with_executable(bin.clone())
        .with_timeout(Duration::from_secs(6))
        .with_keep_cli_sessions_alive(true)
        .with_session(session.clone());

    assert!(!format!("{probe:?}").contains("sk-openai-secret-key"));
    assert!(!format!("{probe:?}").contains("top-secret-ariadusage"));

    let snap1 = probe.fetch(now).await.unwrap();
    assert_eq!(snap1.credits, Some(412.0));
    assert_eq!(snap1.five_hour_percent_left, Some(60));
    assert_eq!(snap1.weekly_percent_left, Some(80));
    assert!(codex_home.join("cursor-reply.txt").exists());
    assert!(codex_home.join("update-skipped.txt").exists());

    // Verify session record was written under <state_dir>/ariadusage/codex/codex-session.json
    let session_record_path = state_dir
        .join("ariadusage")
        .join("codex")
        .join("codex-session.json");
    assert!(session_record_path.exists());

    // Second fetch reuses the warm PTY session without spawning a new process
    let snap2 = probe.fetch(now).await.unwrap();
    assert_eq!(snap2.credits, Some(412.0));
    let inv_log = fs::read_to_string(&invocations_file).unwrap();
    assert_eq!(inv_log.lines().count(), 1);

    // Verify child environment stripped ARIADUSAGE_*, OPENAI_API_KEY, and CODEX_API_KEY
    let first_inv: serde_json::Value =
        serde_json::from_str(inv_log.lines().next().unwrap()).unwrap();
    let env_keys: Vec<&str> = first_inv["env_keys"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(serde_json::Value::as_str)
        .collect();
    assert!(!env_keys.contains(&"ARIADUSAGE_SECRET_TOKEN"));
    assert!(!env_keys.contains(&"OPENAI_API_KEY"));
    assert!(!env_keys.contains(&"CODEX_API_KEY"));
    assert!(env_keys.contains(&"TERM"));

    // Reset warm session and verify session record is cleaned up
    session.reset().await;
    assert!(!session_record_path.exists());

    // 2. First attempt returns "data not available yet" -> probe retries with 70x220 and succeeds on attempt 2
    let _ = fs::remove_file(&invocations_file);
    fs::write(
        &status_file,
        serde_json::to_vec(&json!({
            "status_outputs_by_invocation": [
                "Data not available yet\r\n",
                "Credits: 250 credits\r\n5h limit: [###] 30% left\r\nWeekly limit: [#######] 70% left\r\n"
            ],
            "record_invocation_path": invocations_file.to_string_lossy()
        }))
        .unwrap(),
    )
    .unwrap();

    let one_shot_probe = CodexStatusProbe::new()
        .with_environment(env.clone())
        .with_home_directory(Some(temp.path().to_path_buf()))
        .with_state_directory(&state_dir)
        .with_executable(bin.clone())
        .with_timeout(Duration::from_secs(5));
    let retry_snap = one_shot_probe.fetch(now).await.unwrap();
    assert_eq!(retry_snap.credits, Some(250.0));
    assert_eq!(retry_snap.five_hour_percent_left, Some(30));
    assert_eq!(retry_snap.weekly_percent_left, Some(70));
    let retry_invocations = fs::read_to_string(&invocations_file).unwrap();
    assert_eq!(retry_invocations.lines().count(), 2);

    // 3. Persistent update prompt returns CodexStatusProbeError::UpdateRequired
    fs::write(
        &status_file,
        serde_json::to_vec(&json!({
            "send_update_prompt": true,
            "ignore_update_skip": true
        }))
        .unwrap(),
    )
    .unwrap();
    let update_err = one_shot_probe
        .clone()
        .with_timeout(Duration::from_millis(1200))
        .fetch(now)
        .await
        .unwrap_err();
    assert!(matches!(
        update_err,
        CodexStatusProbeError::UpdateRequired(_)
    ));

    // 4. Launch gate blocks background probe when a launch failure was recorded, while allowing user-initiated probe
    fs::write(
        &status_file,
        serde_json::to_vec(&json!({
            "status_output": "Credits: 100 credits\r\n5h limit: [#####] 50% left\r\n"
        }))
        .unwrap(),
    )
    .unwrap();
    let gate = Arc::new(CodexCLILaunchGate::new());
    gate.record_launch_failure(
        bin.as_path().to_str().unwrap(),
        "The application \"codex\" cannot be opened",
        now,
    );
    let gated_probe = one_shot_probe.with_launch_gate(gate);
    let bg_call = BrokerCall {
        interaction: FetchInteraction::Background,
        cancel: CancellationToken::new(),
        request_id: "bg-pty".to_owned(),
    };
    let bg_err = gated_probe.fetch_with_call(now, bg_call).await.unwrap_err();
    assert!(matches!(bg_err, CodexStatusProbeError::LaunchBlocked(_)));

    let fg_call = BrokerCall {
        interaction: FetchInteraction::UserInitiated,
        cancel: CancellationToken::new(),
        request_id: "fg-pty".to_owned(),
    };
    let fg_snap = gated_probe.fetch_with_call(now, fg_call).await.unwrap();
    assert_eq!(fg_snap.five_hour_percent_left, Some(50));

    // 5. CodexCLIUsageFetcher::load_latest_status delegates to status_probe()
    let fetcher = CodexCLIUsageFetcher::new(env, Some(temp.path().to_path_buf()), state_dir)
        .with_executable(bin);
    let fetcher_status = fetcher.load_latest_status(now, false).await.unwrap();
    assert_eq!(fetcher_status.credits, Some(100.0));
    assert_eq!(fetcher_status.five_hour_percent_left, Some(50));

    let empty_errors = CodexUserFacingErrors::default();
    assert_eq!(empty_errors.credits_hint_copy_text(), None);
}
