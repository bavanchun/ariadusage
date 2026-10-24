// Ported from CodexBar Tests/CodexBarTests/CodexOAuthExpiryPipelineTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

use ariadusage_core::providers::SourceMode;
use ariadusage_engine::brokers::net::NetConfig;
use ariadusage_engine::providers::codex::{
    CodexOAuthError, CodexOAuthFetchError, CodexOAuthFetchStrategy, CodexOAuthUsageFetcher,
};
use jiff::Timestamp;
use std::{collections::BTreeMap, time::Duration};
use tempfile::TempDir;

#[path = "support/https.rs"]
mod https;

// Partial upstream case: CodexOAuthExpiryPipelineTests.swift:166.
// The PAT whoami/usage rows require the PAT transport, outside this PR.
// Keep the complete upstream case MISSING in the mechanical inventory.
#[tokio::test]
async fn oauth_endpoints_distinguish_authentication_failures_from_permission_denials() {
    for endpoint in ["reset", "spend"] {
        for status in [401, 403] {
            let server = https::start_server();
            let mock = server
                .mock_async(|when, then| {
                    when.method(httpmock::Method::GET);
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
            let error = if endpoint == "reset" {
                CodexOAuthUsageFetcher::fetch_rate_limit_reset_credits(
                    &net,
                    "fixture-token",
                    Some("fixture-account"),
                    &env,
                    Some(home.path()),
                    Duration::from_secs(4),
                    Timestamp::now(),
                )
                .await
                .unwrap_err()
            } else {
                CodexOAuthUsageFetcher::fetch_spend_controls_monthly_usage(
                    &net,
                    "fixture-token",
                    "fixture-account",
                    &env,
                    Some(home.path()),
                    Duration::from_secs(4),
                )
                .await
                .unwrap_err()
            };
            assert_eq!(
                error,
                if status == 401 {
                    CodexOAuthFetchError::Unauthorized
                } else {
                    CodexOAuthFetchError::ServerError(403, Some("fixture refusal".to_owned()))
                },
                "endpoint={endpoint}, status={status}"
            );
            assert_eq!(
                CodexOAuthFetchStrategy::should_fallback_on_error(
                    &CodexOAuthError::Fetch(error),
                    SourceMode::Auto
                ),
                status == 401
            );
            mock.assert_calls_async(1).await;
        }
    }
}
