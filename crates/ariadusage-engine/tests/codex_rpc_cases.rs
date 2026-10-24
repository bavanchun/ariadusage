// Ported from CodexBar Tests/CodexBarTests/CodexUsageFetcherFallbackTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

#![cfg(unix)]

#[path = "support/codex_rpc.rs"]
mod rpc;
use ariadusage_engine::providers::codex::*;
use ariadusage_protocol::ProviderId;
use jiff::Timestamp;
use rpc::*;
use serde_json::json;
use std::{collections::BTreeMap, fs, time::Duration};
use tempfile::TempDir;
fn now() -> Timestamp {
    Timestamp::now()
}

// upstream: CodexUsageFetcherFallbackTests.swift:7 missing CLI binary reports install guidance instead of not running
#[tokio::test]
async fn missing_cli_binary_reports_install_guidance_instead_of_not_running() {
    let home = TempDir::new().unwrap();
    let f = CodexCLIUsageFetcher::new(
        BTreeMap::from([
            ("PATH".into(), String::new()),
            (
                "CODEX_CLI_PATH".into(),
                home.path()
                    .join("missing-codex")
                    .to_string_lossy()
                    .into_owned(),
            ),
        ]),
        Some(home.path().into()),
        home.path().into(),
    )
    .with_timeouts(Duration::from_millis(100), Duration::from_millis(100));
    let e = f.load_latest_cli_account_snapshot(now()).await.unwrap_err();
    assert_eq!(
        e,
        CodexCLIFetchError::StatusProbe(CodexStatusProbeError::CodexNotInstalled)
    );
    let message = e.to_string();
    assert!(message.contains("Codex CLI missing"));
    assert!(!message.contains("Codex not running"));
}

// upstream: CodexUsageFetcherFallbackTests.swift:27 CLI usage recovers from RPC decode mismatch body payload
#[test]
fn cli_usage_recovers_from_rpc_decode_mismatch_body_payload() {
    let snapshot = CodexCLIUsageFetcher::recover_codex_rpc_usage_from_error_for_testing(
        DECODE_MISMATCH_BODY_MESSAGE,
        now(),
    )
    .unwrap();
    assert_eq!(snapshot.primary.as_ref().unwrap().used_percent, 4.);
    assert_eq!(snapshot.primary.as_ref().unwrap().window_minutes, Some(300));
    assert_eq!(snapshot.secondary.as_ref().unwrap().used_percent, 19.);
    assert_eq!(
        snapshot.secondary.as_ref().unwrap().window_minutes,
        Some(10080)
    );
    let id = snapshot.identity.unwrap();
    assert_eq!(
        id.account_email.as_deref(),
        Some("prolite-test@example.com")
    );
    assert_eq!(id.login_method.as_deref(), Some("prolite"));
}

// upstream: CodexUsageFetcherFallbackTests.swift:40 CLI credits recover from RPC decode mismatch body payload
#[test]
fn cli_credits_recover_from_rpc_decode_mismatch_body_payload() {
    assert_eq!(
        CodexCLIUsageFetcher::recover_codex_rpc_credits_from_error_for_testing(
            DECODE_MISMATCH_BODY_MESSAGE,
            now()
        )
        .unwrap()
        .remaining,
        0.
    );
}

// upstream: CodexUsageFetcherFallbackTests.swift:47 CLI credits recover from RPC error body when usage windows are unusable
#[tokio::test]
async fn cli_credits_recover_from_rpc_error_body_when_usage_windows_are_unusable() {
    let (_h, f) = mismatch(CREDITS_ONLY_DECODE_MISMATCH_BODY_MESSAGE);
    assert_eq!(f.load_latest_credits(now()).await.unwrap().remaining, 14.5);
    assert_eq!(
        f.load_latest_usage(now()).await.unwrap_err(),
        CodexCLIFetchError::Usage(UsageError::NoRateLimitsFound)
    );
}

// upstream: CodexUsageFetcherFallbackTests.swift:61 CLI usage does not partially recover malformed RPC body without session lane
#[test]
fn cli_usage_does_not_partially_recover_malformed_rpc_body_without_session_lane() {
    assert!(
        CodexCLIUsageFetcher::recover_codex_rpc_usage_from_error_for_testing(
            PARTIAL_DECODE_BODY_MESSAGE,
            now()
        )
        .is_none()
    );
}

// upstream: CodexUsageFetcherFallbackTests.swift:69 CLI usage recovers from RPC body without TTY fallback
#[tokio::test]
async fn cli_usage_recovers_from_rpc_body_without_tty_fallback() {
    let (_h, f) = mismatch(DECODE_MISMATCH_BODY_MESSAGE);
    let v = f.load_latest_usage(now()).await.unwrap();
    assert_eq!(v.primary.as_ref().unwrap().used_percent, 4.);
    assert_eq!(v.primary.as_ref().unwrap().window_minutes, Some(300));
    assert_eq!(v.secondary.as_ref().unwrap().used_percent, 19.);
    assert_eq!(v.secondary.as_ref().unwrap().window_minutes, Some(10080));
}

// upstream: CodexUsageFetcherFallbackTests.swift:83 CLI credits recover from RPC body without TTY fallback
#[tokio::test]
async fn cli_credits_recover_from_rpc_body_without_tty_fallback() {
    let (_h, f) = mismatch(DECODE_MISMATCH_BODY_MESSAGE);
    assert_eq!(f.load_latest_credits(now()).await.unwrap().remaining, 0.);
}

// upstream: CodexUsageFetcherFallbackTests.swift:94 CLI credits load from RPC response without usage windows
#[tokio::test]
async fn cli_credits_load_from_rpc_response_without_usage_windows() {
    let (_h, f) = fixture(
        json!({"result":{"rateLimits":{"credits":{"hasCredits":true,"unlimited":false,"balance":"21"}}}}),
        "pro",
    );
    assert_eq!(f.load_latest_credits(now()).await.unwrap().remaining, 21.);
    assert_eq!(
        f.load_latest_usage(now()).await.unwrap_err(),
        CodexCLIFetchError::Usage(UsageError::NoRateLimitsFound)
    );
}

// upstream: CodexUsageFetcherFallbackTests.swift:108 CLI usage starts app server with current read-only noninteractive arguments
#[tokio::test]
async fn cli_usage_starts_app_server_with_current_read_only_noninteractive_arguments() {
    let (_h, f) = plan_only(Some("pro"), "pro", false, false);
    let v = f.load_latest_usage(now()).await.unwrap();
    assert!(v.primary.is_none());
    assert!(v.secondary.is_none());
    let id = v.identity.as_ref().unwrap();
    assert_eq!(id.account_email.as_deref(), Some("stub@example.com"));
    assert_eq!(id.login_method.as_deref(), Some("pro"));
    assert_eq!(
        UsageLimitsAvailability::resolve(ProviderId::new("codex").unwrap(), Some(&v), None, None),
        UsageLimitsAvailability::Unavailable
    );
}

// upstream: CodexUsageFetcherFallbackTests.swift:123 CLI plan and credits response without usage windows keeps unavailable limits
#[tokio::test]
async fn cli_plan_and_credits_response_without_usage_windows_keeps_unavailable_limits() {
    let (_h, f) = plan_only(Some("pro"), "pro", false, true);
    let snapshot = f.load_latest_cli_account_snapshot(now()).await.unwrap();
    let v = snapshot.usage.unwrap();
    assert!(v.primary.is_none());
    assert!(v.secondary.is_none());
    assert_eq!(
        UsageLimitsAvailability::resolve(ProviderId::new("codex").unwrap(), Some(&v), None, None),
        UsageLimitsAvailability::Unavailable
    );
    assert_eq!(snapshot.credits.unwrap().remaining, 21.);
}

// upstream: CodexUsageFetcherFallbackTests.swift:137 CLI fresh usage plan takes precedence over cached account plan
#[tokio::test]
async fn cli_fresh_usage_plan_takes_precedence_over_cached_account_plan() {
    for plan in [Some("pro"), Some("plus"), Some(""), Some("  "), None] {
        for windows in [false, true] {
            let (_h, f) = plan_only(plan, "plus", windows, false);
            let snapshot = f.load_latest_cli_account_snapshot(now()).await.unwrap();
            let expected = if plan == Some("pro") { "pro" } else { "plus" };
            assert_eq!(
                snapshot.identity.as_ref().unwrap().login_method.as_deref(),
                Some(expected)
            );
            let v = snapshot.usage.unwrap();
            let id = v.identity.as_ref().unwrap();
            assert_eq!(id.login_method.as_deref(), Some(expected));
            assert_eq!(id.account_email.as_deref(), Some("stub@example.com"));
            assert_eq!(
                v.primary.as_ref().map(|w| w.used_percent),
                if windows { Some(12.) } else { None }
            );
            assert_eq!(
                UsageLimitsAvailability::resolve(
                    ProviderId::new("codex").unwrap(),
                    Some(&v),
                    None,
                    None
                )
                .is_unavailable(),
                !windows
            );
        }
    }
}

// upstream: CodexUsageFetcherFallbackTests.swift:158 CLI usage fails when RPC body recovery misses session lane
#[tokio::test]
async fn cli_usage_fails_when_rpc_body_recovery_misses_session_lane() {
    let (_h, f) = mismatch(PARTIAL_DECODE_BODY_MESSAGE);
    assert!(
        f.load_latest_usage(now())
            .await
            .unwrap_err()
            .to_string()
            .contains("Codex connection failed")
    );
}

// upstream: CodexUsageFetcherFallbackTests.swift:173 hung CLI RPC rate limits request reports its timeout
#[tokio::test]
async fn hung_cli_rpc_rate_limits_request_reports_its_timeout() {
    let (h, f) = hung();
    assert_eq!(
        f.load_latest_usage(now()).await.unwrap_err(),
        CodexCLIFetchError::Wire(RPCWireError::Timeout {
            method: "account/rateLimits/read".into()
        })
    );
    assert_eq!(
        fs::read_to_string(h.path().join("requests")).unwrap(),
        "account/rateLimits/read\n"
    );
}

// upstream: CodexUsageFetcherFallbackTests.swift:202 repeated hung CLI RPC requests report their timeouts
#[tokio::test]
async fn repeated_hung_cli_rpc_requests_report_their_timeouts() {
    let (h, f) = hung();
    for attempt in 1..=2 {
        assert_eq!(
            f.load_latest_credits(now()).await.unwrap_err(),
            CodexCLIFetchError::Wire(RPCWireError::Timeout {
                method: "account/rateLimits/read".into()
            })
        );
        assert_eq!(
            fs::read_to_string(h.path().join("requests")).unwrap(),
            "account/rateLimits/read\n".repeat(attempt)
        );
    }
}

// upstream: CodexUsageFetcherFallbackTests.swift:234 hung fixture recognizes both JSON slash encodings
#[tokio::test]
async fn hung_fixture_recognizes_both_json_slash_encodings() {
    use tokio::io::AsyncWriteExt;
    for method in ["account/rateLimits/read", r"account\/rateLimits\/read"] {
        let (home, _f) = hung();
        let mut child = tokio::process::Command::new("/bin/sh")
            .arg(home.path().join("fixture-codex"))
            .arg("app-server")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        child
            .stdin
            .as_mut()
            .unwrap()
            .write_all(format!(r#"{{"id":2,"method":"{method}"}}"#).as_bytes())
            .await
            .unwrap();
        child
            .stdin
            .as_mut()
            .unwrap()
            .write_all(b"\n")
            .await
            .unwrap();
        let path = home.path().join("requests");
        let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
        while fs::read_to_string(&path).ok().as_deref() != Some("account/rateLimits/read\n")
            && tokio::time::Instant::now() < deadline
        {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        let running = child.try_wait().unwrap().is_none();
        child.kill().await.unwrap();
        assert_eq!(
            fs::read_to_string(path).unwrap(),
            "account/rateLimits/read\n"
        );
        assert!(running);
    }
}
