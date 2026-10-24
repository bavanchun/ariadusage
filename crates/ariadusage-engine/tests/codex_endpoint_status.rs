// Ported from CodexBar Tests/CodexBarTests/CodexOAuthExpiryPipelineTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

use ariadusage_core::providers::SourceMode;
use ariadusage_engine::brokers::net::NetConfig;
use ariadusage_engine::providers::codex::{
    CodexCreditLimitSnapshot, CodexOAuthCredentialSource, CodexOAuthError, CodexOAuthFetchError,
    CodexOAuthFetchStrategy, CodexOAuthUsageFetcher, CodexPATCredentials, CodexPATFetchStrategy,
    CodexPATUsageFetcher, PAT_WHOAMI_PATH,
};
use jiff::Timestamp;
use std::{collections::BTreeMap, time::Duration};
use tempfile::TempDir;

#[path = "support/https.rs"]
mod https;

// upstream: CodexOAuthExpiryPipelineTests.swift:166 Codex endpoints distinguish authentication failures from permission denials
#[tokio::test]
async fn codex_endpoints_distinguish_authentication_failures_from_permission_denials() {
    for endpoint in ["reset", "spend", "pat-whoami", "pat-usage"] {
        for status in [401, 403] {
            let server = https::start_server();
            let whoami_ok_mock = if endpoint == "pat-usage" {
                Some(
                    server
                        .mock_async(|when, then| {
                            when.method(httpmock::Method::GET).path(PAT_WHOAMI_PATH);
                            then.status(200)
                                .body(r#"{"chatgpt_account_id":"fixture-account"}"#);
                        })
                        .await,
                )
            } else {
                None
            };
            let mock = server
                .mock_async(|when, then| {
                    if endpoint == "pat-usage" {
                        when.method(httpmock::Method::GET)
                            .path("/backend-api/wham/usage");
                    } else {
                        when.method(httpmock::Method::GET);
                    }
                    then.status(status).body("fixture refusal");
                })
                .await;
            let home = TempDir::new().unwrap();
            std::fs::write(
                home.path().join("config.toml"),
                format!("chatgpt_base_url = \"{}/backend-api\"", server.base_url()),
            )
            .unwrap();
            let env = BTreeMap::from([(
                "CODEX_HOME".to_owned(),
                home.path().to_string_lossy().into_owned(),
            )]);
            let net = https::broker(NetConfig::default());
            let error = match endpoint {
                "reset" => CodexOAuthUsageFetcher::fetch_rate_limit_reset_credits(
                    &net,
                    "fixture-token",
                    Some("fixture-account"),
                    &env,
                    Some(home.path()),
                    Duration::from_secs(4),
                    Timestamp::now(),
                )
                .await
                .unwrap_err(),
                "spend" => CodexOAuthUsageFetcher::fetch_spend_controls_monthly_usage(
                    &net,
                    "fixture-token",
                    "fixture-account",
                    &env,
                    Some(home.path()),
                    Duration::from_secs(4),
                )
                .await
                .unwrap_err(),
                _ => CodexPATUsageFetcher::fetch_usage(
                    &net,
                    &CodexPATCredentials::new("at-fixture", CodexOAuthCredentialSource::CodexHome),
                    Some("1.0.0"),
                    &env,
                    Some(home.path()),
                )
                .await
                .unwrap_err(),
            };
            assert_eq!(
                error,
                if status == 401 {
                    CodexOAuthFetchError::Unauthorized
                } else {
                    CodexOAuthFetchError::ServerError(403, None)
                },
                "endpoint={endpoint}, status={status}"
            );
            let wrapped = CodexOAuthError::Fetch(error);
            assert_eq!(
                CodexOAuthFetchStrategy::should_fallback_on_error(&wrapped, SourceMode::Auto),
                status == 401
            );
            assert_eq!(
                CodexPATFetchStrategy::should_fallback_on_error(&wrapped, SourceMode::Auto),
                status == 401
            );
            if let Some(whoami_mock) = whoami_ok_mock {
                whoami_mock.assert_calls_async(1).await;
            }
            mock.assert_calls_async(1).await;
        }
    }
}

#[tokio::test]
async fn oauth_server_errors_never_leak_response_body_sentinel_in_display_or_debug() {
    const SENTINEL: &str = "sentinel-body-secret-98765";
    for body in [
        SENTINEL.to_owned(),
        format!(r#"{{"error":{{"code":"{SENTINEL}","message":"{SENTINEL}"}}}}"#),
    ] {
        for status in [403_u16, 500, 502] {
            let server = https::start_server();
            let _mock = server
                .mock_async(|when, then| {
                    when.method(httpmock::Method::GET);
                    then.status(status).body(&body);
                })
                .await;
            let home = TempDir::new().unwrap();
            std::fs::write(
                home.path().join("config.toml"),
                format!("chatgpt_base_url = \"{}/backend-api\"", server.base_url()),
            )
            .unwrap();
            let env = BTreeMap::from([(
                "CODEX_HOME".to_owned(),
                home.path().to_string_lossy().into_owned(),
            )]);
            let net = https::broker(NetConfig::default());
            let err = CodexOAuthUsageFetcher::fetch_usage(
                &net,
                "fixture-token",
                Some("fixture-account"),
                &env,
                Some(home.path()),
            )
            .await
            .unwrap_err();
            assert_eq!(err, CodexOAuthFetchError::ServerError(status, None));
            assert!(!err.to_string().contains(SENTINEL));
            assert!(!format!("{err:?}").contains(SENTINEL));
            let fetch_err = err.into_fetch_error();
            assert!(!format!("{fetch_err:?}").contains(SENTINEL));
        }
    }

    let direct = CodexOAuthFetchError::ServerError(502, Some(SENTINEL.to_owned()));
    assert!(!direct.to_string().contains(SENTINEL));
    assert!(!format!("{direct:?}").contains(SENTINEL));
    assert!(format!("{direct:?}").contains("[redacted]"));
    assert!(!format!("{:?}", direct.into_fetch_error()).contains(SENTINEL));
}

#[test]
fn codex_credit_limit_snapshot_recomputes_remaining_when_omitted_on_deserialize() {
    let raw = r#"{
        "title": "Monthly credit limit",
        "used": 250.0,
        "limit": 1000.0,
        "remainingPercent": 75.0,
        "updatedAt": "2026-04-06T00:00:00Z"
    }"#;
    let decoded: CodexCreditLimitSnapshot = serde_json::from_str(raw).unwrap();
    assert_eq!(decoded.used, 250.0);
    assert_eq!(decoded.limit, 1000.0);
    assert_eq!(decoded.remaining, 750.0);
    assert_eq!(decoded.remaining(), 750.0);
}
