// Ported from CodexBar Tests/CodexBarTests/CodexOAuthRequestTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

#[path = "support/https.rs"]
mod https;
use ariadusage_engine::brokers::net::{NetConfig, NetRequest};
use ariadusage_engine::providers::codex::*;
use std::collections::BTreeMap;
use tempfile::TempDir;
fn isolated_home(base: &str) -> (TempDir, BTreeMap<String, String>) {
    let home = TempDir::new().unwrap();
    std::fs::write(
        home.path().join("config.toml"),
        format!("chatgpt_base_url = \"{base}/backend-api\""),
    )
    .unwrap();
    let env = BTreeMap::from([(
        "CODEX_HOME".into(),
        home.path().to_string_lossy().into_owned(),
    )]);
    (home, env)
}

// upstream: CodexOAuthRequestTests.swift:10 authenticated transport disables shared network state
#[tokio::test]
async fn authenticated_transport_disables_shared_network_state() {
    let server = https::start_server();
    let first = server
        .mock_async(|when, then| {
            when.path("/state").cookie_missing("session");
            then.status(200)
                .header("cache-control", "public, max-age=300")
                .header("set-cookie", "session=a; Path=/")
                .body("first");
        })
        .await;
    let net = https::broker(NetConfig::default());
    let url = reqwest::Url::parse(&server.url("/state")).unwrap();
    let declared = https::declared(&url);
    assert_eq!(
        net.send(
            &https::call(),
            &declared,
            NetRequest::new(reqwest::Method::GET, url.clone())
        )
        .await
        .unwrap()
        .body(),
        b"first"
    );
    first.assert_calls_async(1).await;
    first.delete_async().await;
    let next = server
        .mock_async(|when, then| {
            when.path("/state")
                .cookie_missing("session")
                .header_missing("authorization");
            then.status(200).body("second");
        })
        .await;
    assert_eq!(
        net.send(
            &https::call(),
            &declared,
            NetRequest::new(reqwest::Method::GET, url)
        )
        .await
        .unwrap()
        .body(),
        b"second"
    );
    next.assert_calls_async(1).await;
}

// upstream: CodexOAuthRequestTests.swift:21 usage requests fetch distinct cacheable responses for each account
#[tokio::test]
async fn usage_requests_fetch_distinct_cacheable_responses_for_each_account() {
    let server = https::start_server();
    let (home, env) = isolated_home(&server.base_url());
    let net = https::broker(NetConfig::default());
    let payload = |primary, secondary, spark| {
        serde_json::to_string(&serde_json::json!({"rate_limit":{"primary_window":{"used_percent":primary,"reset_at":1766948068_i64,"limit_window_seconds":18000},"secondary_window":{"used_percent":secondary,"reset_at":1767407914_i64,"limit_window_seconds":604800}},"additional_rate_limits":[{"limit_name":"GPT-5.3-Codex-Spark","rate_limit":{"primary_window":{"used_percent":spark,"reset_at":1766948068_i64,"limit_window_seconds":18000}}}]})).unwrap()
    };
    let a = server
        .mock_async(|when, then| {
            when.path("/backend-api/wham/usage")
                .header("chatgpt-account-id", "account-a")
                .header("authorization", "Bearer token-a");
            then.status(200)
                .header("cache-control", "public, max-age=300")
                .body(payload(7, 9, 2));
        })
        .await;
    let b = server
        .mock_async(|when, then| {
            when.path("/backend-api/wham/usage")
                .header("chatgpt-account-id", "account-b")
                .header("authorization", "Bearer token-b");
            then.status(200)
                .header("cache-control", "public, max-age=300")
                .body(payload(100, 63, 4));
        })
        .await;
    let refreshed = CodexOAuthUsageFetcher::fetch_usage(
        &net,
        "token-a",
        Some("account-a"),
        &env,
        Some(home.path()),
    )
    .await
    .unwrap();
    let depleted = CodexOAuthUsageFetcher::fetch_usage(
        &net,
        "token-b",
        Some("account-b"),
        &env,
        Some(home.path()),
    )
    .await
    .unwrap();
    let r = refreshed.rate_limit.unwrap();
    assert_eq!(r.primary_window.unwrap().used_percent, 7);
    assert_eq!(r.secondary_window.unwrap().used_percent, 9);
    let d = depleted.rate_limit.unwrap();
    assert_eq!(d.primary_window.unwrap().used_percent, 100);
    assert_eq!(d.secondary_window.unwrap().used_percent, 63);
    assert_eq!(
        depleted.additional_rate_limits.unwrap()[0]
            .rate_limit
            .as_ref()
            .unwrap()
            .primary_window
            .as_ref()
            .unwrap()
            .used_percent,
        4
    );
    a.assert_calls_async(1).await;
    b.assert_calls_async(1).await;
}

// upstream: CodexOAuthRequestTests.swift:126 token refresh request uses isolated cache policy
#[tokio::test]
async fn token_refresh_request_uses_isolated_cache_policy() {
    let server = https::start_server();
    let mock = server
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST).path("/oauth/token");
            then.status(200)
                .header("cache-control", "public, max-age=300")
                .body(r#"{"access_token":"new-a","refresh_token":"new-r"}"#);
        })
        .await;
    let credentials = CodexOAuthCredentials::new(
        "old-a",
        "old-r",
        None,
        Some("account-a".into()),
        None,
        None,
        CodexOAuthCredentialSource::CodexHome,
        false,
    );
    let net = https::broker(NetConfig::default());
    let refreshed = CodexTokenRefresher::refresh(
        &net,
        https::call(),
        &credentials,
        &server.url("/oauth/token"),
    )
    .await
    .unwrap();
    assert_eq!(refreshed.access_token_str(), "new-a");
    assert_eq!(refreshed.refresh_token_str(), "new-r");
    mock.assert_calls_async(1).await;
}
