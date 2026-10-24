// Ported from CodexBar Tests/CodexBarTests/CodexRateLimitResetCreditsTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

use ariadusage_engine::brokers::net::NetConfig;
use ariadusage_engine::providers::codex::*;
use jiff::Timestamp;
use std::{collections::BTreeMap, time::Duration};
use tempfile::TempDir;
#[path = "support/https.rs"]
mod https;
fn now() -> Timestamp {
    Timestamp::from_second(1_788_134_400).unwrap()
}
fn credit(
    id: &str,
    status: CodexRateLimitResetCreditStatus,
    expires: Option<Timestamp>,
) -> CodexRateLimitResetCredit {
    CodexRateLimitResetCredit::new(
        id,
        "codex_rate_limits",
        status,
        Timestamp::from_second(1_788_000_000).unwrap(),
        expires,
        None,
        None,
        None,
        None,
    )
}
fn home(base: &str) -> (TempDir, BTreeMap<String, String>) {
    let h = TempDir::new().unwrap();
    std::fs::write(
        h.path().join("config.toml"),
        format!("chatgpt_base_url = \"{base}/backend-api\""),
    )
    .unwrap();
    let e = BTreeMap::from([("CODEX_HOME".into(), h.path().to_string_lossy().into_owned())]);
    (h, e)
}

// upstream: CodexRateLimitResetCreditsTests.swift:9 resolves URL from chat GPT config
#[test]
fn resolves_url_from_chat_gpt_config() {
    assert_eq!(
        CodexOAuthUsageFetcher::resolve_rate_limit_reset_credits_url(
            &BTreeMap::new(),
            Some(r#"chatgpt_base_url = "https://chatgpt.com/backend-api/""#),
            None
        )
        .as_str(),
        "https://chatgpt.com/backend-api/wham/rate-limit-reset-credits"
    );
}

// upstream: CodexRateLimitResetCreditsTests.swift:16 request scopes auth and account with bounded timeout
#[tokio::test]
async fn request_scopes_auth_and_account_with_bounded_timeout() {
    let server = https::start_server();
    let (home, env) = home(&server.base_url());
    let mock = server
        .mock_async(|when, then| {
            when.method(httpmock::Method::GET)
                .path("/backend-api/wham/rate-limit-reset-credits")
                .header("authorization", "Bearer test-token")
                .header("chatgpt-account-id", "account-123")
                .header("openai-beta", "codex-1")
                .header("originator", "Codex Desktop");
            then.status(200)
                .body(r#"{"credits":[],"available_count":0}"#);
        })
        .await;
    let net = https::broker(NetConfig::default());
    let snapshot = CodexOAuthUsageFetcher::fetch_rate_limit_reset_credits(
        &net,
        "test-token",
        Some("account-123"),
        &env,
        Some(home.path()),
        Duration::from_secs(4),
        now(),
    )
    .await
    .unwrap();
    assert_eq!(snapshot.available_count, 0);
    mock.assert_calls_async(1).await;
    let req = CodexOAuthUsageFetcher::build_rate_limit_reset_credits_request(
        reqwest::Url::parse("https://chatgpt.com/backend-api/wham/rate-limit-reset-credits")
            .unwrap(),
        "test-token",
        Some("account-123"),
        Duration::from_secs(4),
    )
    .unwrap();
    assert_eq!(req.timeout, Some(Duration::from_secs(4)));
}

// upstream: CodexRateLimitResetCreditsTests.swift:47 rejects negative available count
#[tokio::test]
async fn rejects_negative_available_count() {
    let server = https::start_server();
    let (home, env) = home(&server.base_url());
    let mock = server
        .mock_async(|when, then| {
            when.path("/backend-api/wham/rate-limit-reset-credits");
            then.status(200)
                .body(r#"{"credits":[],"available_count":-1}"#);
        })
        .await;
    let net = https::broker(NetConfig::default());
    assert_eq!(
        CodexOAuthUsageFetcher::fetch_rate_limit_reset_credits(
            &net,
            "test-token",
            None,
            &env,
            Some(home.path()),
            Duration::from_secs(4),
            now()
        )
        .await
        .unwrap_err(),
        CodexOAuthFetchError::InvalidResponse
    );
    mock.assert_calls_async(1).await;
}

// upstream: CodexRateLimitResetCreditsTests.swift:73 decodes credits and skips stale available expiry
#[test]
fn decodes_credits_and_skips_stale_available_expiry() {
    let raw = br##"{
  "credits": [
    {
      "id": "RateLimitResetCredit_expired_available",
      "reset_type": "codex_rate_limits",
      "status": "available",
      "granted_at": "2026-05-18T00:39:53Z",
      "expires_at": "2026-06-17T00:39:53Z"
    },
    {
      "id": "RateLimitResetCredit_later",
      "reset_type": "codex_rate_limits",
      "status": "available",
      "granted_at": "2026-06-18T00:39:53.731630Z",
      "expires_at": "2026-07-18T00:39:53.731630Z",
      "redeem_started_at": null,
      "redeemed_at": null,
      "profile_image_url": "https://example.com/codex.png",
      "profile_user_id": "Codex Team",
      "title": "One free rate limit reset",
      "description": "Thanks for using Codex!"
    },
    {
      "id": "RateLimitResetCredit_earlier",
      "reset_type": "codex_rate_limits",
      "status": "available",
      "granted_at": "2026-06-12T04:03:43.263391Z",
      "expires_at": "2026-07-12T04:03:43.263391Z",
      "redeem_started_at": null,
      "redeemed_at": null,
      "title": "One free rate limit reset",
      "description": "Thanks for using Codex!"
    },
    {
      "id": "RateLimitResetCredit_future_status",
      "reset_type": "codex_rate_limits",
      "status": "future_status",
      "granted_at": "2026-06-12T04:03:43Z",
      "expires_at": "2026-07-10T04:03:43Z",
      "redeem_started_at": null,
      "redeemed_at": null,
      "title": "One free rate limit reset",
      "description": "Thanks for using Codex!"
    }
  ],
  "available_count": 2
}"##;
    let now = "2026-07-01T00:00:00Z".parse::<Timestamp>().unwrap();
    let snapshot = CodexOAuthUsageFetcher::decode_rate_limit_reset_credits(raw, now).unwrap();
    assert_eq!(snapshot.available_count, 2);
    assert_eq!(snapshot.credits.len(), 4);
    assert_eq!(snapshot.credits[0].reset_type, "codex_rate_limits");
    assert_eq!(
        snapshot.credits[3].status,
        CodexRateLimitResetCreditStatus::Unknown("future_status".into())
    );
    assert_eq!(
        snapshot.next_expiring_available_credit().unwrap().id,
        CodexRateLimitResetCredit::stable_id_for_provider_id("RateLimitResetCredit_earlier")
    );
    assert!(
        snapshot
            .credits
            .iter()
            .all(|c| !c.id.contains("RateLimitResetCredit_"))
    );
    let encoded = serde_json::to_vec(&snapshot).unwrap();
    assert!(!String::from_utf8_lossy(&encoded).contains("RateLimitResetCredit_earlier"));
    assert!(!format!("{snapshot:?}").contains("RateLimitResetCredit_earlier"));
    let restored: CodexRateLimitResetCreditsSnapshot = serde_json::from_slice(&encoded).unwrap();
    assert_eq!(
        restored.credits.iter().map(|c| &c.id).collect::<Vec<_>>(),
        snapshot.credits.iter().map(|c| &c.id).collect::<Vec<_>>()
    );
}

// upstream: CodexRateLimitResetCreditsTests.swift:152 available inventory keeps no-expiry credits and sorts deterministically
#[test]
fn available_inventory_keeps_no_expiry_credits_and_sorts_deterministically() {
    let now = now();
    let tied = now.checked_add(Duration::from_secs(3600)).unwrap();
    let snapshot = CodexRateLimitResetCreditsSnapshot::new(
        vec![
            credit("nil-b", CodexRateLimitResetCreditStatus::Available, None),
            credit(
                "expired",
                CodexRateLimitResetCreditStatus::Available,
                Some(now),
            ),
            credit(
                "finite-b",
                CodexRateLimitResetCreditStatus::Available,
                Some(tied),
            ),
            credit(
                "redeemed",
                CodexRateLimitResetCreditStatus::Redeemed,
                Some(now.checked_add(Duration::from_secs(7200)).unwrap()),
            ),
            credit("nil-a", CodexRateLimitResetCreditStatus::Available, None),
            credit(
                "finite-a",
                CodexRateLimitResetCreditStatus::Available,
                Some(tied),
            ),
        ],
        99,
        now,
    );
    let inventory = snapshot.available_inventory(now);
    assert_eq!(inventory.count(), 4);
    let mut finite: Vec<_> = ["finite-a", "finite-b"]
        .map(CodexRateLimitResetCredit::stable_id_for_provider_id)
        .to_vec();
    finite.sort();
    let mut no_expiry: Vec<_> = ["nil-a", "nil-b"]
        .map(CodexRateLimitResetCredit::stable_id_for_provider_id)
        .to_vec();
    no_expiry.sort();
    let expected = finite.iter().chain(no_expiry.iter()).collect::<Vec<_>>();
    assert_eq!(
        inventory.credits.iter().map(|c| &c.id).collect::<Vec<_>>(),
        expected
    );
    assert_eq!(
        inventory.next_expiring_credit().map(|c| &c.id),
        finite.first()
    );
}

// upstream: CodexRateLimitResetCreditsTests.swift:181 provider IDs always hash even when shaped like persisted stable IDs
#[test]
fn provider_ids_always_hash_even_when_shaped_like_persisted_stable_ids() {
    let raw_id = format!("codex-reset-credit-v1-{}", "a".repeat(64));
    let raw=serde_json::to_vec(&serde_json::json!({"credits":[{"id":raw_id,"reset_type":"codex_rate_limits","status":"available","granted_at":"2026-06-18T00:39:53Z","expires_at":null}],"available_count":1})).unwrap();
    let decoded = CodexOAuthUsageFetcher::decode_rate_limit_reset_credits(&raw, now()).unwrap();
    let id = &decoded.credits[0].id;
    let expected = CodexRateLimitResetCredit::stable_id_for_provider_id(&raw_id);
    assert_eq!(id, &expected);
    assert_ne!(id, &raw_id);
    let model = credit(&raw_id, CodexRateLimitResetCreditStatus::Available, None);
    assert_eq!(model.id, expected);
    assert_ne!(model.id, raw_id);
    let restored: CodexRateLimitResetCredit =
        serde_json::from_slice(&serde_json::to_vec(&model).unwrap()).unwrap();
    assert_eq!(restored.id, expected);
    let a = credit(
        "ordinary-provider-id",
        CodexRateLimitResetCreditStatus::Available,
        None,
    );
    let b = credit(
        "ordinary-provider-id",
        CodexRateLimitResetCreditStatus::Available,
        None,
    );
    assert_eq!(a.id, b.id);
    assert_ne!(a.id, "ordinary-provider-id");
}

// upstream: CodexRateLimitResetCreditsTests.swift:221 reset credit GET preserves transport cancellation
#[tokio::test]
async fn reset_credit_get_preserves_transport_cancellation() {
    let server = https::start_server();
    let (home, env) = home(&server.base_url());
    let mock = server
        .mock_async(|when, then| {
            when.method(httpmock::Method::GET)
                .path("/backend-api/wham/rate-limit-reset-credits");
            then.status(200)
                .delay(Duration::from_secs(5))
                .body(r#"{"credits":[],"available_count":0}"#);
        })
        .await;
    let net = https::broker(NetConfig::default());
    let call = https::call();
    let cancellation = call.cancel.clone();
    let request = CodexOAuthUsageFetcher::fetch_rate_limit_reset_credits_with_call(
        &net,
        "test-token",
        Some("account-123"),
        &env,
        Some(home.path()),
        Duration::from_secs(4),
        now(),
        Some(&call),
    );
    let canceller = async {
        tokio::time::timeout(Duration::from_secs(2), async {
            while mock.calls_async().await == 0 {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("reset-credit request must reach the local endpoint");
        cancellation.cancel();
    };
    let (response, ()) = tokio::join!(request, canceller);
    assert_eq!(response.unwrap_err(), CodexOAuthFetchError::Cancelled);
}
