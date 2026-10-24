// Ported from CodexBar Tests/CodexBarTests/CodexOAuthTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/CodexOAuthCredentialReadTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/CodexOAuthCreditLimitTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/CodexOAuthExpiryPipelineTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/CodexOAuthRequestTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/CodexOAuthResetCreditFetchTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/CodexOAuthCredentialsStorePermissionsTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/CodexAdditionalRateLimitsTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/CodexExtraUsageCostTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/CodexIdentityResolverTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/CodexPlanFormattingTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/CodexRateLimitResetCreditsTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/CodexSpendControlNumberTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/CodexSpendControlsMonthlyUsageTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/CodexUnlimitedCreditsTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/CodexWorkspaceBalanceTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

#[path = "support/https.rs"]
mod https;

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use ariadusage_core::model::cost::ProviderCostSnapshot;
use ariadusage_core::model::identity::ProviderIdentitySnapshot;
use ariadusage_core::model::snapshot::UsageSnapshot;
use ariadusage_core::model::window::RateWindow;
use ariadusage_core::pipeline::{
    FetchContext, FetchError, FetchInteraction, FetchPhase, FetchRuntime, FetchStrategy,
};
use ariadusage_core::providers::SourceMode;
use ariadusage_engine::brokers::net::{NetBroker, NetConfig};
use ariadusage_engine::providers::codex::{
    AdditionalRateLimit, CODEX_CLI_STRATEGY_ID, CODEX_OAUTH_NATIVE_REFRESH_CLI_STRATEGY_ID,
    CODEX_OAUTH_STRATEGY_ID, CODEX_PAT_STRATEGY_ID, CodexAdditionalRateLimitMapper,
    CodexCreditLimitSnapshot, CodexExtraUsageCost, CodexFetchOutcome, CodexIdentity,
    CodexIdentityResolver, CodexNativeCredentialOwnerIdentity, CodexOAuthCredentialSource,
    CodexOAuthCredentials, CodexOAuthCredentialsError, CodexOAuthCredentialsStore, CodexOAuthError,
    CodexOAuthFetchError, CodexOAuthFetchStrategy, CodexOAuthNativeRefreshCLIStrategy,
    CodexOAuthUsageFetcher, CodexPATFetchStrategy, CodexPATWhoami, CodexPlanFormatting,
    CodexProviderPlan, CodexRateLimitResetCredit, CodexRateLimitResetCreditStatus,
    CodexRateLimitResetCreditsSnapshot, CodexRateWindowNormalizer, CodexRefreshError,
    CodexSpendControlNumber, CodexSpendControlsMonthlyUsageGate,
    CodexSpendControlsMonthlyUsageResponse, CodexStrategyConfig, CodexTokenRefresher,
    CodexUIErrorMapper, CodexUsageDataSource, CodexUsageLane, CodexUsageResponse, CreditEvent,
    CreditsSnapshot, PlanType, SpendControlLimitSnapshot, icon_windows, no_data_message,
    plan_utilization_series, resolve_usage_strategy, visible_windows, weekly_caps_session,
};
use ariadusage_protocol::metric::SourceKind;
use ariadusage_protocol::secret::SecretString;
use ariadusage_protocol::{Confidence, ProviderId};
use jiff::Timestamp;
use serde_json::{Value, json};
use tempfile::TempDir;
use tokio_util::sync::CancellationToken;

fn base64url_no_pad(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b0 = chunk[0];
        let b1 = chunk.get(1).copied().unwrap_or(0);
        let b2 = chunk.get(2).copied().unwrap_or(0);
        out.push(TABLE[(b0 >> 2) as usize] as char);
        out.push(TABLE[(((b0 & 0x03) << 4) | (b1 >> 4)) as usize] as char);
        if chunk.len() > 1 {
            out.push(TABLE[(((b1 & 0x0f) << 2) | (b2 >> 6)) as usize] as char);
        }
        if chunk.len() > 2 {
            out.push(TABLE[(b2 & 0x3f) as usize] as char);
        }
    }
    out
}

fn make_jwt(claims: &Value) -> String {
    make_jwt_raw(&serde_json::to_vec(claims).unwrap())
}

fn make_jwt_raw(payload_bytes: &[u8]) -> String {
    let header = base64url_no_pad(r#"{"alg":"none","typ":"JWT"}"#.as_bytes());
    let payload = base64url_no_pad(payload_bytes);
    format!("{header}.{payload}.sig")
}

fn make_fetch_context(source_mode: SourceMode) -> FetchContext {
    FetchContext {
        provider: ProviderId::new("codex").unwrap(),
        runtime: FetchRuntime::Cli,
        interaction: FetchInteraction::UserInitiated,
        phase: FetchPhase::Regular,
        request_id: "test-oauth-req".to_owned(),
        source_mode,
        env: BTreeMap::new(),
        include_credits: false,
        selected_token_account: None,
        cancel: CancellationToken::new(),
    }
}

fn make_credentials_with_id_token(id_token: Option<String>) -> CodexOAuthCredentials {
    CodexOAuthCredentials::new(
        "synthetic-access-token",
        "synthetic-refresh-token",
        id_token.map(SecretString::new),
        Some("acct-test-123".to_owned()),
        Some(Timestamp::now()),
        None,
        CodexOAuthCredentialSource::CodexHome,
        false,
    )
}

// ============================================================================
// CodexOAuthTests.swift
// ============================================================================

#[test]
fn parses_tokens_data_and_legacy_camel_case() {
    let id_token = make_jwt(&json!({
        "email": "inventor@example.test",
        "https://api.openai.com/auth": {
            "chatgpt_plan_type": "pro",
            "chatgpt_account_id": "acct-from-jwt"
        }
    }));
    let data = serde_json::to_vec(&json!({
        "OPENAI_API_KEY": null,
        "tokens": {
            "access_token": "tok-access-1",
            "refresh_token": "tok-refresh-1",
            "id_token": id_token,
            "account_id": "acct-explicit"
        },
        "last_refresh": "2026-04-10T12:00:00Z"
    }))
    .unwrap();

    let creds =
        CodexOAuthCredentialsStore::parse(&data, CodexOAuthCredentialSource::CodexHome).unwrap();
    assert_eq!(creds.access_token_str(), "tok-access-1");
    assert_eq!(creds.refresh_token_str(), "tok-refresh-1");
    assert_eq!(creds.account_id.as_deref(), Some("acct-explicit"));
    assert!(!creds.is_api_key);
    assert!(creds.last_refresh.is_some());

    let camel_data = serde_json::to_vec(&json!({
        "OPENAI_API_KEY": null,
        "tokens": {
            "accessToken": "access-token",
            "refreshToken": "refresh-token",
            "idToken": "id-token",
            "accountId": "account-123"
        },
        "last_refresh": "2025-12-20T12:34:56Z"
    }))
    .unwrap();
    let camel_creds =
        CodexOAuthCredentialsStore::parse(&camel_data, CodexOAuthCredentialSource::CodexHome)
            .unwrap();
    assert_eq!(camel_creds.access_token_str(), "access-token");
    assert_eq!(camel_creds.refresh_token_str(), "refresh-token");
    assert_eq!(camel_creds.id_token_str(), Some("id-token"));
    assert_eq!(camel_creds.account_id.as_deref(), Some("account-123"));
    assert!(camel_creds.last_refresh.is_some());
}

#[test]
fn parses_api_key_only_and_reset_credit_token_load_ignores_api_key_beside_oauth_tokens() {
    let data = serde_json::to_vec(&json!({
        "OPENAI_API_KEY": "synthetic-openai-key-value"
    }))
    .unwrap();

    let creds =
        CodexOAuthCredentialsStore::parse(&data, CodexOAuthCredentialSource::CodexHome).unwrap();
    assert_eq!(creds.access_token_str(), "synthetic-openai-key-value");
    assert_eq!(creds.refresh_token_str(), "");
    assert!(creds.id_token.is_none());
    assert!(creds.account_id.is_none());
    assert!(creds.is_api_key);
    assert!(!creds.needs_refresh_at(Timestamp::now()));

    let err = CodexOAuthCredentialsStore::parse_oauth_tokens(
        &data,
        CodexOAuthCredentialSource::CodexHome,
    )
    .unwrap_err();
    assert_eq!(err, CodexOAuthCredentialsError::MissingTokens);

    let temp = TempDir::new().unwrap();
    let codex_home = temp.path().join(".codex");
    fs::create_dir_all(&codex_home).unwrap();
    let both_data = serde_json::to_vec(&json!({
        "OPENAI_API_KEY": "sk-test",
        "tokens": {
            "access_token": "oauth-access-token",
            "refresh_token": "oauth-refresh-token",
            "account_id": "account-123"
        },
        "last_refresh": "2026-07-01T12:00:00Z"
    }))
    .unwrap();
    CodexOAuthCredentialsStore::write_private_file_for_testing(
        &codex_home.join("auth.json"),
        &both_data,
    )
    .unwrap();
    let mut env = BTreeMap::new();
    env.insert(
        "CODEX_HOME".to_owned(),
        codex_home.to_string_lossy().into_owned(),
    );
    let oauth_creds =
        CodexOAuthCredentialsStore::load_oauth_tokens(&env, Some(temp.path())).unwrap();
    assert_eq!(oauth_creds.access_token_str(), "oauth-access-token");
    assert_eq!(oauth_creds.refresh_token_str(), "oauth-refresh-token");
    assert_eq!(oauth_creds.account_id.as_deref(), Some("account-123"));
}

#[test]
fn throws_on_missing_tokens() {
    let data = br#"{"tokens":{"access_token":"","refresh_token":"tok"}}"#;
    let err =
        CodexOAuthCredentialsStore::parse(data, CodexOAuthCredentialSource::CodexHome).unwrap_err();
    assert_eq!(err, CodexOAuthCredentialsError::MissingTokens);
}

#[test]
fn needs_refresh_when_last_refresh_older_than_eight_days() {
    let now = "2026-04-18T12:00:00Z".parse::<Timestamp>().unwrap();
    let old = now.checked_sub(Duration::from_secs(9 * 86_400)).unwrap();
    let recent = now.checked_sub(Duration::from_secs(2 * 86_400)).unwrap();

    let stale = CodexOAuthCredentials::new(
        "tok",
        "ref",
        None,
        None,
        Some(old),
        None,
        CodexOAuthCredentialSource::LegacyCodexHome,
        false,
    );
    assert!(stale.needs_refresh_at(now));

    let fresh = CodexOAuthCredentials::new(
        "tok",
        "ref",
        None,
        None,
        Some(recent),
        None,
        CodexOAuthCredentialSource::LegacyCodexHome,
        false,
    );
    assert!(!fresh.needs_refresh_at(now));
}

#[test]
fn parses_opencode_openai_oauth_entry() {
    let now_ms = 1_776_000_000_000i64;
    let data = serde_json::to_vec(&json!({
        "openai": {
            "type": "oauth",
            "access": "opencode-access-tok",
            "refresh": "opencode-refresh-tok",
            "expires": now_ms,
            "accountId": "acct-opencode-1"
        }
    }))
    .unwrap();

    let creds = CodexOAuthCredentialsStore::parse_open_code(&data).unwrap();
    assert_eq!(creds.access_token_str(), "opencode-access-tok");
    assert_eq!(creds.refresh_token_str(), "opencode-refresh-tok");
    assert_eq!(creds.account_id.as_deref(), Some("acct-opencode-1"));
    assert_eq!(creds.source, CodexOAuthCredentialSource::OpenCode);
    assert_eq!(
        creds.expires_at,
        Some(Timestamp::from_millisecond(now_ms).unwrap())
    );
}

#[test]
fn resolves_and_validates_usage_url_variants() {
    let empty_env = BTreeMap::new();
    let default_url = CodexOAuthUsageFetcher::resolve_usage_url(&empty_env, None, None);
    assert_eq!(
        default_url.as_str(),
        "https://chatgpt.com/backend-api/wham/usage"
    );

    let chatgpt_with_backend = CodexOAuthUsageFetcher::resolve_usage_url(
        &empty_env,
        Some("chatgpt_base_url = \"https://chatgpt.com/backend-api/\"\n"),
        None,
    );
    assert_eq!(
        chatgpt_with_backend.as_str(),
        "https://chatgpt.com/backend-api/wham/usage"
    );

    let chat_openai = CodexOAuthUsageFetcher::resolve_usage_url(
        &empty_env,
        Some("chatgpt_base_url = \"https://chat.openai.com\"\n"),
        None,
    );
    assert_eq!(
        chat_openai.as_str(),
        "https://chat.openai.com/backend-api/wham/usage"
    );

    let api_openai = CodexOAuthUsageFetcher::resolve_usage_url(
        &empty_env,
        Some("chatgpt_base_url = \"https://api.openai.com\"\n"),
        None,
    );
    assert_eq!(
        api_openai.as_str(),
        "https://api.openai.com/api/codex/usage"
    );

    // Override validation accepts OpenAI/ChatGPT HTTPS hosts and loopback for testing,
    // and rejects arbitrary hosts, cleartext non-loopback HTTP, link-local, or query strings.
    assert!(
        CodexOAuthUsageFetcher::validate_chatgpt_base_url("https://chatgpt.com/backend-api/")
            .is_ok()
    );
    assert!(CodexOAuthUsageFetcher::validate_chatgpt_base_url("https://chat.openai.com").is_ok());
    assert!(CodexOAuthUsageFetcher::validate_chatgpt_base_url("https://api.openai.com").is_ok());
    assert!(
        CodexOAuthUsageFetcher::validate_chatgpt_base_url("https://127.0.0.1:8443/backend-api")
            .is_ok()
    );
    assert!(
        CodexOAuthUsageFetcher::validate_chatgpt_base_url("http://127.0.0.1:8080/backend-api")
            .is_ok()
    );
    assert!(
        CodexOAuthUsageFetcher::validate_chatgpt_base_url("https://evil.invalid/backend-api")
            .is_err()
    );
    assert!(
        CodexOAuthUsageFetcher::validate_chatgpt_base_url("http://chatgpt.com/backend-api")
            .is_err()
    );
    assert!(
        CodexOAuthUsageFetcher::validate_chatgpt_base_url("https://169.254.169.254/backend-api")
            .is_err()
    );
    assert!(
        CodexOAuthUsageFetcher::validate_chatgpt_base_url("https://chatgpt.com/backend-api?leak=1")
            .is_err()
    );
}

#[test]
fn decodes_usage_response_and_normalizes_windows_across_all_combinations() {
    let now = "2026-04-18T12:00:00Z".parse::<Timestamp>().unwrap();
    let id_token = make_jwt(&json!({
        "email": "user@example.test",
        "https://api.openai.com/auth": {
            "chatgpt_plan_type": "prolite"
        }
    }));
    let creds = make_credentials_with_id_token(Some(id_token));

    // Primary has 7-day window (604800s), secondary has 5-hour window (18000s) -> swaps!
    // Also API plan_type ("pro") wins over JWT id_token plan ("prolite").
    let raw = serde_json::to_vec(&json!({
        "plan_type": "pro",
        "rate_limit": {
            "primary_window": {
                "used_percent": 40,
                "reset_at": 1_800_000_000,
                "limit_window_seconds": 604_800
            },
            "secondary_window": {
                "used_percent": 15,
                "reset_at": 1_799_500_000,
                "limit_window_seconds": 18_000
            }
        },
        "credits": {
            "has_credits": true,
            "unlimited": false,
            "balance": "42.5"
        }
    }))
    .unwrap();

    let outcome =
        CodexOAuthFetchStrategy::map_result_for_testing(&raw, &creds, None, false, true, now)
            .unwrap();
    let primary = outcome.usage.primary.as_ref().unwrap();
    let secondary = outcome.usage.secondary.as_ref().unwrap();
    assert_eq!(primary.used_percent, 15.0);
    assert_eq!(primary.window_minutes, Some(300));
    assert_eq!(secondary.used_percent, 40.0);
    assert_eq!(secondary.window_minutes, Some(10_080));
    assert_eq!(outcome.usage.data_confidence, Confidence::Exact);

    let identity = outcome.usage.identity.as_ref().unwrap();
    assert_eq!(identity.account_email.as_deref(), Some("user@example.test"));
    assert_eq!(identity.login_method.as_deref(), Some("pro"));

    let credits = outcome.credits.as_ref().unwrap();
    assert_eq!(credits.remaining, 42.5);
    assert!(credits.balance_read_succeeded);
    assert_eq!(credits.credits_available, Some(true));

    // Free weekly-only window maps into secondary:
    let free_weekly = serde_json::to_vec(&json!({
        "plan_type": "free",
        "rate_limit": {
            "primary_window": {
                "used_percent": 0,
                "reset_at": 1_775_468_693,
                "limit_window_seconds": 604_800
            },
            "secondary_window": null
        }
    }))
    .unwrap();
    let free_snap = CodexOAuthFetchStrategy::map_usage_for_testing(&free_weekly, &creds, now)
        .unwrap()
        .unwrap();
    assert!(free_snap.primary.is_none());
    assert_eq!(free_snap.secondary.as_ref().unwrap().used_percent, 0.0);
    assert_eq!(
        free_snap.secondary.as_ref().unwrap().window_minutes,
        Some(10_080)
    );

    // Single session window stays primary:
    let single_session = serde_json::to_vec(&json!({
        "rate_limit": {
            "primary_window": {
                "used_percent": 9,
                "reset_at": 1_766_948_068,
                "limit_window_seconds": 18_000
            },
            "secondary_window": null
        }
    }))
    .unwrap();
    let session_snap = CodexOAuthFetchStrategy::map_usage_for_testing(&single_session, &creds, now)
        .unwrap()
        .unwrap();
    assert_eq!(session_snap.primary.as_ref().unwrap().used_percent, 9.0);
    assert_eq!(
        session_snap.primary.as_ref().unwrap().window_minutes,
        Some(300)
    );
    assert!(session_snap.secondary.is_none());

    // Unknown single window (540m) stays primary whether in primary_window or secondary_window:
    for field in ["primary_window", "secondary_window"] {
        let mut rl = serde_json::Map::new();
        rl.insert(
            field.to_owned(),
            json!({
                "used_percent": 17,
                "reset_at": 1_766_948_068,
                "limit_window_seconds": 32_400
            }),
        );
        let raw_unknown = serde_json::to_vec(&json!({ "rate_limit": rl })).unwrap();
        let snap = CodexOAuthFetchStrategy::map_usage_for_testing(&raw_unknown, &creds, now)
            .unwrap()
            .unwrap();
        assert_eq!(snap.primary.as_ref().unwrap().used_percent, 17.0);
        assert_eq!(snap.primary.as_ref().unwrap().window_minutes, Some(540));
        assert!(snap.secondary.is_none());
    }

    // Reversed weekly (10080m) and unknown (540m) windows swap:
    let reversed_unknown = serde_json::to_vec(&json!({
        "rate_limit": {
            "primary_window": {
                "used_percent": 43,
                "reset_at": 1_767_407_914,
                "limit_window_seconds": 604_800
            },
            "secondary_window": {
                "used_percent": 17,
                "reset_at": 1_766_948_068,
                "limit_window_seconds": 32_400
            }
        }
    }))
    .unwrap();
    let swapped_unknown =
        CodexOAuthFetchStrategy::map_usage_for_testing(&reversed_unknown, &creds, now)
            .unwrap()
            .unwrap();
    assert_eq!(swapped_unknown.primary.as_ref().unwrap().used_percent, 17.0);
    assert_eq!(
        swapped_unknown.primary.as_ref().unwrap().window_minutes,
        Some(540)
    );
    assert_eq!(
        swapped_unknown.secondary.as_ref().unwrap().used_percent,
        43.0
    );
    assert_eq!(
        swapped_unknown.secondary.as_ref().unwrap().window_minutes,
        Some(10_080)
    );

    // Unknown primary (540m) and Session secondary (300m) do NOT swap (upstream only swaps Weekly+Session or Weekly+Unknown):
    let unknown_w = RateWindow::new(17.0, Some(540), None, None, None, false).unwrap();
    let session_w = RateWindow::new(9.0, Some(300), None, None, None, false).unwrap();
    let (norm_p, norm_s) = CodexRateWindowNormalizer::normalize(Some(unknown_w), Some(session_w));
    assert_eq!(norm_p.unwrap().window_minutes, Some(540));
    assert_eq!(norm_s.unwrap().window_minutes, Some(300));

    // No windows -> map_usage_for_testing returns Ok(None):
    let no_windows = serde_json::to_vec(&json!({
        "rate_limit": {
            "primary_window": null,
            "secondary_window": null
        }
    }))
    .unwrap();
    assert!(
        CodexOAuthFetchStrategy::map_usage_for_testing(&no_windows, &creds, now)
            .unwrap()
            .is_none()
    );
}

#[test]
fn decodes_malformed_windows_and_credits_while_preserving_valid_lanes() {
    let now = "2026-04-18T12:00:00Z".parse::<Timestamp>().unwrap();
    let creds = make_credentials_with_id_token(None);

    // 1. Valid primary + malformed secondary -> keeps primary, confidence Unknown
    let bad_secondary = serde_json::to_vec(&json!({
        "plan_type": "plus",
        "rate_limit": {
            "primary_window": {
                "used_percent": 18,
                "reset_at": 1_766_948_068,
                "limit_window_seconds": 18_000
            },
            "secondary_window": {
                "used_percent": "bad",
                "reset_at": 1_767_407_914,
                "limit_window_seconds": 604_800
            }
        }
    }))
    .unwrap();
    let out1 = CodexOAuthFetchStrategy::map_result_for_testing(
        &bad_secondary,
        &creds,
        None,
        false,
        true,
        now,
    )
    .unwrap();
    assert_eq!(out1.usage.primary.as_ref().unwrap().used_percent, 18.0);
    assert!(out1.usage.secondary.is_none());
    assert_eq!(out1.usage.data_confidence, Confidence::Unknown);

    // 2. Malformed primary + valid weekly secondary -> keeps weekly in secondary
    let bad_primary_weekly = serde_json::to_vec(&json!({
        "rate_limit": {
            "primary_window": {
                "used_percent": "bad",
                "reset_at": 1_766_948_068,
                "limit_window_seconds": 18_000
            },
            "secondary_window": {
                "used_percent": 43,
                "reset_at": 1_767_407_914,
                "limit_window_seconds": 604_800
            }
        }
    }))
    .unwrap();
    let out2 = CodexOAuthFetchStrategy::map_result_for_testing(
        &bad_primary_weekly,
        &creds,
        None,
        false,
        true,
        now,
    )
    .unwrap();
    assert!(out2.usage.primary.is_none());
    assert_eq!(out2.usage.secondary.as_ref().unwrap().used_percent, 43.0);
    assert_eq!(
        out2.usage.secondary.as_ref().unwrap().window_minutes,
        Some(10_080)
    );

    // 3. Malformed weekly primary + valid reversed session secondary -> promotes session to primary
    let bad_weekly_valid_session = serde_json::to_vec(&json!({
        "rate_limit": {
            "primary_window": {
                "used_percent": "bad",
                "reset_at": 1_767_407_914,
                "limit_window_seconds": 604_800
            },
            "secondary_window": {
                "used_percent": 18,
                "reset_at": 1_766_948_068,
                "limit_window_seconds": 18_000
            }
        }
    }))
    .unwrap();
    let out3 = CodexOAuthFetchStrategy::map_result_for_testing(
        &bad_weekly_valid_session,
        &creds,
        None,
        false,
        true,
        now,
    )
    .unwrap();
    assert_eq!(out3.usage.primary.as_ref().unwrap().used_percent, 18.0);
    assert_eq!(
        out3.usage.primary.as_ref().unwrap().window_minutes,
        Some(300)
    );
    assert!(out3.usage.secondary.is_none());

    // 4. Valid weekly primary + malformed reversed session secondary -> moves weekly to secondary
    let valid_weekly_bad_session = serde_json::to_vec(&json!({
        "rate_limit": {
            "primary_window": {
                "used_percent": 43,
                "reset_at": 1_767_407_914,
                "limit_window_seconds": 604_800
            },
            "secondary_window": {
                "used_percent": "bad",
                "reset_at": 1_766_948_068,
                "limit_window_seconds": 18_000
            }
        }
    }))
    .unwrap();
    let out4 = CodexOAuthFetchStrategy::map_result_for_testing(
        &valid_weekly_bad_session,
        &creds,
        None,
        false,
        true,
        now,
    )
    .unwrap();
    assert!(out4.usage.primary.is_none());
    assert_eq!(out4.usage.secondary.as_ref().unwrap().used_percent, 43.0);
    assert_eq!(
        out4.usage.secondary.as_ref().unwrap().window_minutes,
        Some(10_080)
    );

    // 5. Ignores malformed credits balance array while keeping usage
    let bad_credits = serde_json::to_vec(&json!({
        "plan_type": "prolite",
        "rate_limit": {
            "primary_window": {
                "used_percent": 22,
                "reset_at": 1_766_948_068,
                "limit_window_seconds": 18_000
            }
        },
        "credits": {
            "has_credits": false,
            "unlimited": false,
            "balance": []
        }
    }))
    .unwrap();
    let decoded = CodexOAuthUsageFetcher::decode_usage_response(&bad_credits).unwrap();
    assert_eq!(
        decoded.plan_type,
        Some(PlanType::Unknown("prolite".to_owned()))
    );
    assert!(!decoded.credits.as_ref().unwrap().has_credits);
    assert_eq!(decoded.credits.as_ref().unwrap().balance, None);
    let snap5 = CodexOAuthFetchStrategy::map_usage_for_testing(&bad_credits, &creds, now)
        .unwrap()
        .unwrap();
    assert_eq!(snap5.primary.as_ref().unwrap().used_percent, 22.0);
}

#[test]
fn credits_only_and_reset_credits_only_responses_succeed_without_windows() {
    let now = "2026-04-18T12:00:00Z".parse::<Timestamp>().unwrap();
    let creds = make_credentials_with_id_token(None);

    let credits_only = serde_json::to_vec(&json!({
        "plan_type": "plus",
        "credits": {
            "has_credits": true,
            "unlimited": false,
            "balance": "14.5"
        }
    }))
    .unwrap();
    let outcome = CodexOAuthFetchStrategy::map_result_for_testing(
        &credits_only,
        &creds,
        None,
        false,
        true,
        now,
    )
    .unwrap();
    assert!(outcome.usage.primary.is_none());
    assert!(outcome.usage.secondary.is_none());
    assert_eq!(outcome.credits.as_ref().unwrap().remaining, 14.5);
    assert_eq!(
        outcome.usage.provider_cost.as_ref().unwrap().balance,
        Some(14.5)
    );
    assert_eq!(outcome.source_label, "oauth");

    let empty_payload = serde_json::to_vec(&json!({"plan_type": "plus"})).unwrap();
    let err = CodexOAuthFetchStrategy::map_result_for_testing(
        &empty_payload,
        &creds,
        None,
        false,
        true,
        now,
    )
    .unwrap_err();
    assert_eq!(err, CodexOAuthError::NoRateLimitsFound);

    // Empty reset credits do not mask missing OAuth usage:
    let empty_resets = CodexRateLimitResetCreditsSnapshot::new(Vec::new(), 0, now);
    let err_empty_resets = CodexOAuthFetchStrategy::map_result_for_testing(
        &empty_payload,
        &creds,
        Some(empty_resets),
        false,
        true,
        now,
    )
    .unwrap_err();
    assert_eq!(err_empty_resets, CodexOAuthError::NoRateLimitsFound);

    // App enrichment flag can rescue empty usage before reset credit enrichment:
    let rescued = CodexOAuthFetchStrategy::map_result_for_testing(
        &empty_payload,
        &creds,
        None,
        true,
        true,
        now,
    )
    .unwrap();
    assert!(rescued.usage.primary.is_none());
    assert_eq!(rescued.strategy_id, CODEX_OAUTH_STRATEGY_ID);

    // With available reset credits, empty windows still succeed:
    let reset_credit = CodexRateLimitResetCredit::new(
        "raw-grant-1",
        "five_hour",
        CodexRateLimitResetCreditStatus::Available,
        now,
        None,
        None,
        None,
        None,
        None,
    );
    let reset_snap = CodexRateLimitResetCreditsSnapshot::new(vec![reset_credit], 1, now);
    let outcome_with_resets = CodexOAuthFetchStrategy::map_result_for_testing(
        &empty_payload,
        &creds,
        Some(reset_snap),
        false,
        true,
        now,
    )
    .unwrap();
    assert_eq!(
        outcome_with_resets
            .codex_reset_credits
            .as_ref()
            .unwrap()
            .available_inventory(now)
            .count(),
        1
    );
}

#[test]
fn token_refresher_failure_classification_and_login_guidance() {
    let revoked = CodexTokenRefresher::refresh_failure_error_for_testing(
        400,
        br#"{"error":"invalid_grant"}"#,
    );
    assert_eq!(revoked, CodexRefreshError::Revoked);

    let invalid_req = CodexTokenRefresher::refresh_failure_error_for_testing(
        400,
        br#"{"error":"invalid_request"}"#,
    );
    assert_eq!(
        invalid_req,
        CodexRefreshError::InvalidResponse("Status 400".to_owned())
    );

    let expired = CodexTokenRefresher::refresh_failure_error_for_testing(
        401,
        br#"{"error":{"code":"refresh_token_expired"}}"#,
    );
    assert_eq!(expired, CodexRefreshError::Expired);

    let reused = CodexTokenRefresher::refresh_failure_error_for_testing(
        401,
        br#"{"error":{"code":"refresh_token_reused"}}"#,
    );
    assert_eq!(reused, CodexRefreshError::Reused);

    assert!(
        CodexOAuthCredentialsError::NativeRefreshRequired
            .to_string()
            .contains("codex login")
    );
    assert!(
        CodexOAuthCredentialsError::ReadOnlySource
            .to_string()
            .contains("codex login")
    );
    assert!(
        CodexOAuthFetchError::Unauthorized
            .to_string()
            .contains("codex login")
    );
}

#[test]
fn pat_result_mapping_preserves_whoami_and_confidence() {
    let now = "2026-04-18T12:00:00Z".parse::<Timestamp>().unwrap();
    let whoami = CodexPATWhoami {
        account_id: None,
        email: Some("pat-user@example.test".to_owned()),
        plan_type: Some("team".to_owned()),
    };
    let raw = serde_json::to_vec(&json!({
        "rate_limit": {
            "primary_window": {
                "used_percent": 33,
                "reset_at": 1_800_000_000,
                "limit_window_seconds": 18_000
            }
        }
    }))
    .unwrap();

    let outcome = CodexPATFetchStrategy::map_result_for_testing(&raw, Some(&whoami), now).unwrap();
    assert_eq!(outcome.source_label, "pat");
    assert_eq!(outcome.strategy_id, CODEX_PAT_STRATEGY_ID);
    assert_eq!(outcome.usage.data_confidence, Confidence::Exact);
    let id = outcome.usage.identity.as_ref().unwrap();
    assert_eq!(id.account_email.as_deref(), Some("pat-user@example.test"));
    assert_eq!(id.login_method.as_deref(), Some("team"));
}

#[test]
fn visible_windows_weekly_caps_session_and_hides_expired_exhausted_windows() {
    let now = "2026-04-18T12:00:00Z".parse::<Timestamp>().unwrap();
    let future = now.checked_add(Duration::from_secs(3600)).unwrap();
    let past = now.checked_sub(Duration::from_secs(60)).unwrap();

    let session = RateWindow::new(25.0, Some(300), Some(future), None, None, false).unwrap();
    let weekly_exhausted =
        RateWindow::new(100.0, Some(10_080), Some(future), None, None, false).unwrap();
    assert!(weekly_caps_session(Some(&weekly_exhausted), now));

    let snap = UsageSnapshot::new(
        Some(session),
        Some(weekly_exhausted),
        None,
        None,
        None,
        Vec::new(),
        None,
        None,
        now,
        None,
        Confidence::Exact,
    )
    .unwrap();
    let visible = visible_windows(&snap, now);
    assert_eq!(visible.len(), 2);
    assert_eq!(visible[0].used_percent, 100.0);
    assert_eq!(visible[1].used_percent, 100.0);

    // Exhausted window with reset_at == 0 (epoch <= now) is hidden by visible_windows:
    let raw_zero_reset = serde_json::to_vec(&json!({
        "rate_limit": {
            "primary_window": {
                "used_percent": 100,
                "reset_at": 0,
                "limit_window_seconds": 18_000
            },
            "secondary_window": {
                "used_percent": 25,
                "reset_at": 1_800_500_000,
                "limit_window_seconds": 604_800
            }
        }
    }))
    .unwrap();
    let creds = make_credentials_with_id_token(None);
    let zero_reset_snap =
        CodexOAuthFetchStrategy::map_usage_for_testing(&raw_zero_reset, &creds, now)
            .unwrap()
            .unwrap();
    assert_eq!(
        zero_reset_snap.primary.as_ref().unwrap().resets_at,
        Some(Timestamp::UNIX_EPOCH)
    );
    let visible_zero = visible_windows(&zero_reset_snap, now);
    assert_eq!(visible_zero.len(), 1);
    assert_eq!(visible_zero[0].used_percent, 25.0);

    let session2 = RateWindow::new(25.0, Some(300), Some(future), None, None, false).unwrap();
    let weekly_past = RateWindow::new(100.0, Some(10_080), Some(past), None, None, false).unwrap();
    assert!(!weekly_caps_session(Some(&weekly_past), now));
    let snap2 = UsageSnapshot::new(
        Some(session2),
        Some(weekly_past),
        None,
        None,
        None,
        Vec::new(),
        None,
        None,
        now,
        None,
        Confidence::Exact,
    )
    .unwrap();
    let visible2 = visible_windows(&snap2, now);
    assert_eq!(visible2.len(), 1);
    assert_eq!(visible2[0].used_percent, 25.0);

    let (icon_p, icon_s) = icon_windows(&snap2, now);
    assert!(icon_p.is_some());
    assert!(icon_s.is_none());

    let monthly = RateWindow::new(50.0, Some(43_200), Some(future), None, None, false).unwrap();
    let snap_monthly = UsageSnapshot::new(
        Some(monthly),
        None,
        None,
        None,
        None,
        Vec::new(),
        None,
        None,
        now,
        None,
        Confidence::Exact,
    )
    .unwrap();
    assert_eq!(
        plan_utilization_series(&snap_monthly),
        BTreeSet::from([CodexUsageLane::Monthly])
    );
}

#[test]
fn ui_error_mapper_formats_codex_parse_failures() {
    assert_eq!(
        CodexUIErrorMapper::user_facing_message(Some("Data not available yet")).as_deref(),
        Some("Could not parse Codex status; will retry shortly.")
    );
    assert_eq!(
        CodexUIErrorMapper::user_facing_message(Some(
            "Could not parse Codex status; will retry shortly."
        ))
        .as_deref(),
        Some("Could not parse Codex status; will retry shortly.")
    );
}

#[test]
fn resolve_usage_strategy_and_no_data_message_match_upstream() {
    assert_eq!(
        resolve_usage_strategy(CodexUsageDataSource::Auto, false, true),
        ariadusage_engine::providers::codex::CodexUsageStrategy {
            data_source: CodexUsageDataSource::Pat
        }
    );
    assert_eq!(
        resolve_usage_strategy(CodexUsageDataSource::Auto, true, false),
        ariadusage_engine::providers::codex::CodexUsageStrategy {
            data_source: CodexUsageDataSource::OAuth
        }
    );
    assert_eq!(
        resolve_usage_strategy(CodexUsageDataSource::Auto, false, false),
        ariadusage_engine::providers::codex::CodexUsageStrategy {
            data_source: CodexUsageDataSource::Cli
        }
    );

    let mut env = BTreeMap::new();
    env.insert("CODEX_HOME".to_owned(), "/tmp/custom-codex-home".to_owned());
    assert_eq!(
        no_data_message(&env, None),
        "No Codex sessions found in /tmp/custom-codex-home/sessions or /tmp/custom-codex-home/archived_sessions."
    );
}

// ============================================================================
// CodexOAuthCredentialReadTests.swift & CodexOAuthCredentialsStorePermissionsTests.swift
// ============================================================================

#[test]
fn missing_unreadable_and_malformed_auth_json_errors() {
    let temp = TempDir::new().unwrap();
    let missing_home = temp.path().join("missing-codex-home");
    let mut env = BTreeMap::new();
    env.insert(
        "CODEX_HOME".to_owned(),
        missing_home.to_string_lossy().into_owned(),
    );
    assert_eq!(
        CodexOAuthCredentialsStore::load_oauth_tokens(&env, Some(temp.path())).unwrap_err(),
        CodexOAuthCredentialsError::NotFound
    );

    let unreadable_home = temp.path().join("unreadable-codex-home");
    fs::create_dir_all(unreadable_home.join("auth.json")).unwrap();
    env.insert(
        "CODEX_HOME".to_owned(),
        unreadable_home.to_string_lossy().into_owned(),
    );
    assert_eq!(
        CodexOAuthCredentialsStore::load_oauth_tokens(&env, Some(temp.path())).unwrap_err(),
        CodexOAuthCredentialsError::Unreadable
    );

    assert!(matches!(
        CodexOAuthCredentialsStore::parse(b"not-json", CodexOAuthCredentialSource::CodexHome)
            .unwrap_err(),
        CodexOAuthCredentialsError::DecodeFailed(_)
    ));
    assert_eq!(
        CodexOAuthCredentialsStore::parse(b"[]", CodexOAuthCredentialSource::CodexHome)
            .unwrap_err(),
        CodexOAuthCredentialsError::DecodeFailed("Invalid JSON".to_owned())
    );
}

#[test]
fn jwt_account_id_and_organization_fallbacks() {
    // 1. Namespaced OpenAI JWT auth claim
    let namespaced_jwt = make_jwt(&json!({
        "https://api.openai.com/auth": { "chatgpt_account_id": "acct-namespaced" }
    }));
    let creds1 = CodexOAuthCredentialsStore::parse(
        &serde_json::to_vec(&json!({
            "tokens": {
                "access_token": "opaque-access",
                "refresh_token": "refresh",
                "id_token": namespaced_jwt
            }
        }))
        .unwrap(),
        CodexOAuthCredentialSource::CodexHome,
    )
    .unwrap();
    assert_eq!(creds1.account_id.as_deref(), Some("acct-namespaced"));

    // 2. Direct chatgpt_account_id claim
    let direct_jwt = make_jwt(&json!({ "chatgpt_account_id": "acct-direct" }));
    let creds2 = CodexOAuthCredentialsStore::parse(
        &serde_json::to_vec(&json!({
            "tokens": {
                "access_token": "opaque-access",
                "refresh_token": "refresh",
                "id_token": direct_jwt
            }
        }))
        .unwrap(),
        CodexOAuthCredentialSource::CodexHome,
    )
    .unwrap();
    assert_eq!(creds2.account_id.as_deref(), Some("acct-direct"));

    // 3. Whitespace account_id falls back to JWT organization claim
    let org_jwt = make_jwt(&json!({
        "organizations": [{ "id": "org-from-jwt" }]
    }));
    let creds3 = CodexOAuthCredentialsStore::parse(
        &serde_json::to_vec(&json!({
            "tokens": {
                "access_token": org_jwt,
                "refresh_token": "refresh",
                "account_id": "  \n"
            }
        }))
        .unwrap(),
        CodexOAuthCredentialSource::CodexHome,
    )
    .unwrap();
    assert_eq!(creds3.account_id.as_deref(), Some("org-from-jwt"));

    // 4. First non-blank organization is selected
    let multi_org_jwt = make_jwt(&json!({
        "organizations": [{ "id": "  " }, { "id": "org-later" }, { "id": "" }]
    }));
    let creds4 = CodexOAuthCredentialsStore::parse(
        &serde_json::to_vec(&json!({
            "tokens": {
                "access_token": multi_org_jwt,
                "refresh_token": "refresh"
            }
        }))
        .unwrap(),
        CodexOAuthCredentialSource::CodexHome,
    )
    .unwrap();
    assert_eq!(creds4.account_id.as_deref(), Some("org-later"));
}

#[test]
fn jwt_expiry_variants_and_five_minute_native_refresh_window() {
    let now = "2026-04-18T12:00:00Z".parse::<Timestamp>().unwrap();

    // Valid future native JWT expiry overrides stale refresh timestamp
    let future_jwt = make_jwt(&json!({ "exp": 4_102_444_800i64 }));
    let future_creds = CodexOAuthCredentialsStore::parse(
        &serde_json::to_vec(&json!({
            "tokens": { "access_token": future_jwt, "refresh_token": "native-refresh" },
            "last_refresh": "2000-01-01T00:00:00Z"
        }))
        .unwrap(),
        CodexOAuthCredentialSource::CodexHome,
    )
    .unwrap();
    assert_eq!(
        future_creds.expires_at,
        Some(Timestamp::from_second(4_102_444_800).unwrap())
    );
    assert!(!future_creds.needs_refresh_at(now));

    // Expired native JWT (exp = 0 and exp = 1) overrides fresh refresh timestamp
    for exp in [0i64, 1i64] {
        let exp_jwt = make_jwt(&json!({ "exp": exp }));
        let exp_creds = CodexOAuthCredentialsStore::parse(
            &serde_json::to_vec(&json!({
                "tokens": { "access_token": exp_jwt, "refresh_token": "native-refresh" },
                "last_refresh": now.to_string()
            }))
            .unwrap(),
            CodexOAuthCredentialSource::CodexHome,
        )
        .unwrap();
        assert_eq!(
            exp_creds.expires_at,
            Some(Timestamp::from_second(exp).unwrap())
        );
        assert!(exp_creds.needs_refresh_at(now));
    }

    // Native OAuth refreshes within the 5-minute window (2 minutes left), while external OpenCode does not
    let two_mins_left = now.checked_add(Duration::from_secs(120)).unwrap();
    let native_near = CodexOAuthCredentials::new(
        "native-access",
        "native-refresh",
        None,
        None,
        None,
        Some(two_mins_left),
        CodexOAuthCredentialSource::CodexHome,
        false,
    );
    let external_near = CodexOAuthCredentials::new(
        "external-access",
        "external-refresh",
        None,
        None,
        None,
        Some(two_mins_left),
        CodexOAuthCredentialSource::OpenCode,
        false,
    );
    assert!(native_near.needs_refresh_at(now));
    assert!(!external_near.needs_refresh_at(now));

    // Boolean, fractional, integral float, out-of-range integer, and empty header/signature fall back to last_refresh
    let parts: Vec<&str> = future_jwt.split('.').collect();
    let malformed_jwts = vec![
        make_jwt(&json!({ "exp": true })),
        make_jwt(&json!({ "exp": 1.5 })),
        make_jwt_raw(br#"{"exp":4102444800.0}"#),
        make_jwt_raw(br#"{"exp":9223372036854775808}"#),
        make_jwt(&json!({ "exp": i64::MAX })),
        format!(".{}.{}", parts[1], parts[2]),
        format!("{}.{}.", parts[0], parts[1]),
    ];
    for token in malformed_jwts {
        let creds = CodexOAuthCredentialsStore::parse(
            &serde_json::to_vec(&json!({
                "tokens": { "access_token": token, "refresh_token": "native-refresh" },
                "last_refresh": "2000-01-01T00:00:00Z"
            }))
            .unwrap(),
            CodexOAuthCredentialSource::CodexHome,
        )
        .unwrap();
        assert_eq!(creds.expires_at, None);
        assert!(creds.needs_refresh_at(now));
    }
}

#[test]
fn credential_precedence_and_external_source_isolation() {
    let temp = TempDir::new().unwrap();
    let home = temp.path();
    let codex_dir = home.join(".codex");
    let config_codex_dir = home.join(".config/codex");
    let opencode_dir = home.join(".local/share/opencode");
    fs::create_dir_all(&codex_dir).unwrap();
    fs::create_dir_all(&config_codex_dir).unwrap();
    fs::create_dir_all(&opencode_dir).unwrap();

    let legacy_auth = config_codex_dir.join("auth.json");
    let opencode_auth = opencode_dir.join("auth.json");

    CodexOAuthCredentialsStore::write_private_file_for_testing(
        &legacy_auth,
        br#"{"tokens":{"access_token":"legacy-tok","refresh_token":"legacy-ref"}}"#,
    )
    .unwrap();
    CodexOAuthCredentialsStore::write_private_file_for_testing(
        &opencode_auth,
        br#"{"openai":{"type":"oauth","access":"open-tok","refresh":"open-ref"}}"#,
    )
    .unwrap();

    let mut env = BTreeMap::new();
    env.insert("HOME".to_owned(), home.to_string_lossy().into_owned());

    // Without allow_external_sources, missing ~/.codex/auth.json returns NotFound
    let err = CodexOAuthCredentialsStore::load_for_usage(&env, false, Some(home)).unwrap_err();
    assert_eq!(err, CodexOAuthCredentialsError::NotFound);

    // With allow_external_sources, falls back to ~/.config/codex/auth.json before OpenCode
    let creds = CodexOAuthCredentialsStore::load_for_usage(&env, true, Some(home)).unwrap();
    assert_eq!(creds.access_token_str(), "legacy-tok");
    assert_eq!(creds.source, CodexOAuthCredentialSource::LegacyCodexHome);

    // Legacy API key in ~/.config/codex/auth.json is rejected by external OAuth fallback
    CodexOAuthCredentialsStore::write_private_file_for_testing(
        &legacy_auth,
        br#"{"OPENAI_API_KEY":"legacy-api-key"}"#,
    )
    .unwrap();
    let _ = fs::remove_file(&opencode_auth);
    assert_eq!(
        CodexOAuthCredentialsStore::load_for_usage(&env, true, Some(home)).unwrap_err(),
        CodexOAuthCredentialsError::NotFound
    );

    // OpenCode API key is rejected as MissingTokens
    assert_eq!(
        CodexOAuthCredentialsStore::parse_open_code(
            br#"{"openai":{"type":"api","key":"open-code-api-key"}}"#
        )
        .unwrap_err(),
        CodexOAuthCredentialsError::MissingTokens
    );

    // External fallback does not mask unreadable, malformed, or tokenless native ~/.codex/auth.json
    CodexOAuthCredentialsStore::write_private_file_for_testing(
        &opencode_auth,
        br#"{"openai":{"type":"oauth","access":"open-tok","refresh":"open-ref"}}"#,
    )
    .unwrap();
    let native_auth = codex_dir.join("auth.json");
    CodexOAuthCredentialsStore::write_private_file_for_testing(&native_auth, b"not-json").unwrap();
    assert!(matches!(
        CodexOAuthCredentialsStore::load_for_usage(&env, true, Some(home)).unwrap_err(),
        CodexOAuthCredentialsError::DecodeFailed(_)
    ));
    CodexOAuthCredentialsStore::write_private_file_for_testing(&native_auth, br#"{"tokens":{}}"#)
        .unwrap();
    assert_eq!(
        CodexOAuthCredentialsStore::load_for_usage(&env, true, Some(home)).unwrap_err(),
        CodexOAuthCredentialsError::MissingTokens
    );
    let _ = fs::remove_file(&native_auth);
    fs::create_dir_all(&native_auth).unwrap();
    assert_eq!(
        CodexOAuthCredentialsStore::load_for_usage(&env, true, Some(home)).unwrap_err(),
        CodexOAuthCredentialsError::Unreadable
    );
    let _ = fs::remove_dir_all(&native_auth);

    // If CODEX_HOME is explicitly set, external sources are NEVER consulted even if allow_external_sources is true
    let custom_codex_home = home.join("isolated-codex-home");
    fs::create_dir_all(&custom_codex_home).unwrap();
    env.insert(
        "CODEX_HOME".to_owned(),
        custom_codex_home.to_string_lossy().into_owned(),
    );
    let err2 = CodexOAuthCredentialsStore::load_for_usage(&env, true, Some(home)).unwrap_err();
    assert_eq!(err2, CodexOAuthCredentialsError::NotFound);
}

#[cfg(unix)]
#[test]
fn save_writes_owner_only_permissions_0600_and_rejects_read_only_sources() {
    use std::os::unix::fs::PermissionsExt;

    let temp = TempDir::new().unwrap();
    let codex_home = temp.path().join(".codex");
    let mut env = BTreeMap::new();
    env.insert(
        "CODEX_HOME".to_owned(),
        codex_home.to_string_lossy().into_owned(),
    );

    let saved_ts = Timestamp::from_second(1_700_000_000).unwrap();
    let creds = CodexOAuthCredentials::new(
        "saved-access-tok",
        "saved-refresh-tok",
        None,
        Some("acct-saved".to_owned()),
        Some(saved_ts),
        None,
        CodexOAuthCredentialSource::CodexHome,
        false,
    );
    CodexOAuthCredentialsStore::save(&creds, &env, Some(temp.path())).unwrap();

    let auth_path = codex_home.join("auth.json");
    let mode = fs::metadata(&auth_path).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600);

    let reloaded = CodexOAuthCredentialsStore::load_oauth_tokens(&env, Some(temp.path())).unwrap();
    assert_eq!(reloaded.last_refresh, Some(saved_ts));

    // Read-only sources (OpenCode, LegacyCodexHome) are refused by save:
    let open_code_creds = CodexOAuthCredentials::new(
        "open-access",
        "open-refresh",
        None,
        None,
        None,
        None,
        CodexOAuthCredentialSource::OpenCode,
        false,
    );
    assert_eq!(
        CodexOAuthCredentialsStore::save(&open_code_creds, &env, Some(temp.path())).unwrap_err(),
        CodexOAuthCredentialsError::ReadOnlySource
    );
}

#[tokio::test]
async fn load_credentials_retries_bounded_and_cancelled_fetch_does_not_read() {
    let now = "2026-04-18T12:00:00Z".parse::<Timestamp>().unwrap();
    let stale_time = now.checked_sub(Duration::from_secs(9 * 86_400)).unwrap();
    let fresh_time = now;

    let mut attempts = 0usize;
    let loaded = CodexOAuthFetchStrategy::load_credentials_with_reader(true, now, || {
        attempts += 1;
        if attempts == 1 {
            return Err(CodexOAuthCredentialsError::DecodeFailed(
                "partial write".to_owned(),
            ));
        }
        if attempts == 2 {
            return Ok(CodexOAuthCredentials::new(
                "stale-tok",
                "ref",
                None,
                None,
                Some(stale_time),
                None,
                CodexOAuthCredentialSource::CodexHome,
                false,
            ));
        }
        Ok(CodexOAuthCredentials::new(
            "fresh-tok",
            "ref",
            None,
            None,
            Some(fresh_time),
            None,
            CodexOAuthCredentialSource::CodexHome,
            false,
        ))
    })
    .await
    .unwrap();

    assert_eq!(attempts, 3);
    assert_eq!(loaded.access_token_str(), "fresh-tok");

    // Bounded retries preserve the final error after 3 reads
    for expected_err in [
        CodexOAuthCredentialsError::NotFound,
        CodexOAuthCredentialsError::Unreadable,
        CodexOAuthCredentialsError::DecodeFailed("Invalid JSON".to_owned()),
        CodexOAuthCredentialsError::MissingTokens,
    ] {
        let mut count = 0usize;
        let err_clone = expected_err.clone();
        let res = CodexOAuthFetchStrategy::load_credentials_with_reader(true, now, || {
            count += 1;
            Err(err_clone.clone())
        })
        .await;
        assert_eq!(count, 3);
        assert_eq!(res.unwrap_err(), expected_err);
    }

    // Cancelled FetchContext returns FetchError::Cancelled without reading credentials
    let temp = TempDir::new().unwrap();
    let net = Arc::new(NetBroker::new(NetConfig::default()).unwrap());
    let cfg = CodexStrategyConfig::new(net, BTreeMap::new(), temp.path(), temp.path());
    let strategy = CodexOAuthFetchStrategy::new(cfg);
    let cancelled_ctx = make_fetch_context(SourceMode::Oauth);
    cancelled_ctx.cancel.cancel();
    let fetch_err = strategy.fetch(&cancelled_ctx).await.unwrap_err();
    assert!(matches!(fetch_err, FetchError::Cancelled));
    assert!(!strategy.should_fallback(&fetch_err, &cancelled_ctx));
}

// ============================================================================
// CodexOAuthExpiryPipelineTests.swift
// ============================================================================

#[test]
fn prepare_credentials_for_usage_and_fallback_rules() {
    let now = "2026-04-18T12:00:00Z".parse::<Timestamp>().unwrap();
    let past_exp = now.as_second() - 60;
    let expired_jwt = make_jwt(&json!({ "exp": past_exp }));

    let native_expired = CodexOAuthCredentials::new(
        expired_jwt.as_str(),
        "ref",
        None,
        None,
        Some(now),
        Some(Timestamp::from_second(past_exp).unwrap()),
        CodexOAuthCredentialSource::CodexHome,
        false,
    );
    let err =
        CodexOAuthFetchStrategy::prepare_credentials_for_usage(&native_expired, now).unwrap_err();
    assert_eq!(err, CodexOAuthCredentialsError::NativeRefreshRequired);

    // Explicit OAuth mode falls back ONLY on NativeRefreshRequired
    assert!(CodexOAuthFetchStrategy::should_fallback_on_error(
        &CodexOAuthError::Credentials(CodexOAuthCredentialsError::NativeRefreshRequired),
        SourceMode::Oauth,
    ));
    assert!(!CodexOAuthFetchStrategy::should_fallback_on_error(
        &CodexOAuthError::Fetch(CodexOAuthFetchError::Unauthorized),
        SourceMode::Oauth,
    ));

    // External read-only expired credentials return ReadOnlySource and never fall back
    for source in [
        CodexOAuthCredentialSource::OpenCode,
        CodexOAuthCredentialSource::LegacyCodexHome,
    ] {
        let external_expired = CodexOAuthCredentials::new(
            "tok",
            "",
            None,
            None,
            None,
            Some(Timestamp::from_second(past_exp).unwrap()),
            source,
            false,
        );
        let ext_err =
            CodexOAuthFetchStrategy::prepare_credentials_for_usage(&external_expired, now)
                .unwrap_err();
        assert_eq!(ext_err, CodexOAuthCredentialsError::ReadOnlySource);
        assert!(!CodexOAuthFetchStrategy::should_fallback_on_error(
            &CodexOAuthError::Credentials(CodexOAuthCredentialsError::ReadOnlySource),
            SourceMode::Auto,
        ));
    }

    // Valid read-only credentials pass through without refresh
    let valid_external = CodexOAuthCredentials::new(
        "valid-access",
        "external-refresh",
        None,
        None,
        Some(now),
        Some(now.checked_add(Duration::from_secs(3600)).unwrap()),
        CodexOAuthCredentialSource::OpenCode,
        false,
    );
    let prepared =
        CodexOAuthFetchStrategy::prepare_credentials_for_usage(&valid_external, now).unwrap();
    assert_eq!(prepared.access_token_str(), "valid-access");

    // Auto mode fallback matrix:
    for fallback_err in [
        CodexOAuthError::Fetch(CodexOAuthFetchError::Unauthorized),
        CodexOAuthError::Credentials(CodexOAuthCredentialsError::NotFound),
        CodexOAuthError::Credentials(CodexOAuthCredentialsError::Unreadable),
        CodexOAuthError::Credentials(CodexOAuthCredentialsError::MissingTokens),
        CodexOAuthError::Credentials(CodexOAuthCredentialsError::NativeRefreshRequired),
        CodexOAuthError::Refresh(CodexRefreshError::Expired),
        CodexOAuthError::Refresh(CodexRefreshError::Revoked),
        CodexOAuthError::Refresh(CodexRefreshError::Reused),
    ] {
        assert!(CodexOAuthFetchStrategy::should_fallback_on_error(
            &fallback_err,
            SourceMode::Auto,
        ));
    }
    for non_fallback_err in [
        CodexOAuthError::NoRateLimitsFound,
        CodexOAuthError::Credentials(CodexOAuthCredentialsError::DecodeFailed("bad".to_owned())),
        CodexOAuthError::Credentials(CodexOAuthCredentialsError::ReadOnlySource),
        CodexOAuthError::Credentials(CodexOAuthCredentialsError::Cancelled),
        CodexOAuthError::Fetch(CodexOAuthFetchError::InvalidResponse),
        CodexOAuthError::Fetch(CodexOAuthFetchError::ServerError(500, None)),
        CodexOAuthError::Fetch(CodexOAuthFetchError::NetworkError("offline".to_owned())),
        CodexOAuthError::Fetch(CodexOAuthFetchError::Cancelled),
        CodexOAuthError::Refresh(CodexRefreshError::NetworkError("offline".to_owned())),
        CodexOAuthError::Refresh(CodexRefreshError::InvalidResponse("400".to_owned())),
        CodexOAuthError::Refresh(CodexRefreshError::Cancelled),
    ] {
        assert!(!CodexOAuthFetchStrategy::should_fallback_on_error(
            &non_fallback_err,
            SourceMode::Auto,
        ));
    }
}

#[test]
fn native_refresh_cli_strategy_availability_checks_workspace_binary_and_source() {
    let temp = TempDir::new().unwrap();
    let codex_home = temp.path().join(".codex");
    fs::create_dir_all(&codex_home).unwrap();

    let now = "2026-04-18T12:00:00Z".parse::<Timestamp>().unwrap();
    let past_exp = now.as_second() - 120;
    let expired_jwt = make_jwt(&json!({ "exp": past_exp }));

    let auth_bytes = serde_json::to_vec(&json!({
        "tokens": {
            "access_token": expired_jwt,
            "refresh_token": "ref-tok"
        }
    }))
    .unwrap();
    CodexOAuthCredentialsStore::write_private_file_for_testing(
        &codex_home.join("auth.json"),
        &auth_bytes,
    )
    .unwrap();

    let mut env = BTreeMap::new();
    env.insert(
        "CODEX_HOME".to_owned(),
        codex_home.to_string_lossy().into_owned(),
    );

    let net = Arc::new(NetBroker::new(NetConfig::default()).unwrap());
    let mut cfg = CodexStrategyConfig::new(net, env, temp.path(), temp.path());
    cfg.clock = Arc::new(move || now);

    let strategy =
        CodexOAuthNativeRefreshCLIStrategy::new(cfg.clone()).with_binary_resolver(|| true);
    assert!(strategy.is_available_sync(SourceMode::Oauth));
    assert!(!strategy.is_available_sync(SourceMode::Auto));

    // Unavailable when Codex CLI binary is missing
    let no_bin_strategy =
        CodexOAuthNativeRefreshCLIStrategy::new(cfg.clone()).with_binary_resolver(|| false);
    assert!(!no_bin_strategy.is_available_sync(SourceMode::Oauth));

    // When managed_workspace_account_id is set, native refresh CLI strategy is disabled
    let mut workspace_cfg = cfg;
    workspace_cfg.managed_workspace_account_id = Some("ws-managed-1".to_owned());
    let ws_strategy = CodexOAuthNativeRefreshCLIStrategy::new(workspace_cfg.clone())
        .with_binary_resolver(|| true);
    assert!(!ws_strategy.is_available_sync(SourceMode::Oauth));

    let plan = CodexProviderPlan::new(workspace_cfg);
    assert_eq!(
        plan.resolve_strategy_ids(SourceMode::Auto),
        vec![CODEX_PAT_STRATEGY_ID, CODEX_OAUTH_STRATEGY_ID]
    );
    assert_eq!(
        plan.resolve_strategy_ids(SourceMode::Oauth),
        vec![
            CODEX_OAUTH_STRATEGY_ID,
            CODEX_OAUTH_NATIVE_REFRESH_CLI_STRATEGY_ID
        ]
    );
    assert_eq!(
        plan.resolve_strategy_ids(SourceMode::Cli),
        vec![CODEX_CLI_STRATEGY_ID]
    );
}

// ============================================================================
// CodexOAuthCreditLimitTests.swift & CodexSpendControlNumberTests.swift
// ============================================================================

#[test]
fn spend_control_number_parses_doubles_and_integers_permissively() {
    let cases: &[(&str, Option<i64>)] = &[
        ("1782864000", Some(1_782_864_000)),
        ("1782864000.9", Some(1_782_864_000)),
        ("-1.9", Some(-1)),
        (r#"" 1782864000 ""#, Some(1_782_864_000)),
        ("9223372036854775808", None),
        ("1e100", None),
        ("-1e100", None),
        ("null", None),
        ("true", None),
    ];
    for (raw, expected) in cases {
        let payload = format!(r#"{{"reset_at":{raw},"limit":" 100 ","used":12.5}}"#);
        let snapshot: SpendControlLimitSnapshot = serde_json::from_str(&payload).unwrap();
        assert_eq!(snapshot.resets_at, *expected, "raw={raw}");
        assert_eq!(snapshot.limit, Some(100.0));
        assert_eq!(snapshot.used, Some(12.5));
    }

    let obj: serde_json::Map<String, Value> = serde_json::from_value(json!({
        "f_num": 12.5,
        "i_num": 42,
        "s_num": " 99.75 ",
        "bad": "not-a-number"
    }))
    .unwrap();
    assert_eq!(CodexSpendControlNumber::double(&obj, "f_num"), Some(12.5));
    assert_eq!(CodexSpendControlNumber::double(&obj, "i_num"), Some(42.0));
    assert_eq!(CodexSpendControlNumber::double(&obj, "s_num"), Some(99.75));
    assert_eq!(CodexSpendControlNumber::double(&obj, "bad"), None);
}

#[test]
fn resolved_individual_limit_precedence_and_credit_limit_mapping() {
    let now = "2026-04-18T12:00:00Z".parse::<Timestamp>().unwrap();
    let creds = make_credentials_with_id_token(None);

    // 1. Root > rate_limit > spend_control
    let raw = serde_json::to_vec(&json!({
        "plan_type": "team",
        "individual_limit": {
            "limit": 500,
            "used": 125,
            "reset_at": 1_800_000_000
        },
        "rate_limit": {
            "individual_limit": {
                "limit": 300,
                "used": 100
            }
        },
        "spend_control": {
            "individual_limit": {
                "limit": 200,
                "used": 50
            }
        }
    }))
    .unwrap();

    let outcome =
        CodexOAuthFetchStrategy::map_result_for_testing(&raw, &creds, None, false, true, now)
            .unwrap();
    let credits = outcome.credits.as_ref().unwrap();
    assert!(!credits.balance_read_succeeded);
    // display_remaining prefers codex_credit_limit.remaining when no workspace balance is present!
    assert_eq!(credits.display_remaining(), Some(375.0));
    let limit = credits.codex_credit_limit.as_ref().unwrap();
    assert_eq!(limit.limit, 500.0);
    assert_eq!(limit.used, 125.0);
    assert_eq!(limit.remaining_percent, 75.0);
    assert_eq!(
        limit.resets_at,
        Some(Timestamp::from_second(1_800_000_000).unwrap())
    );

    // 2. rate_limit.individual_limit wins over spend_control.individual_limit
    let rate_limit_wins = serde_json::to_vec(&json!({
        "plan_type": "team",
        "rate_limit": {
            "primary_window": null,
            "secondary_window": null,
            "individual_limit": {
                "limit": 100_000,
                "used": "7761",
                "remaining_percent": 92.239,
                "resets_at": 1_782_864_000
            }
        },
        "spend_control": {
            "individual_limit": {
                "limit": "1000",
                "used": "36.79748725891113",
                "remaining_percent": 96,
                "reset_at": 1_788_220_800
            }
        }
    }))
    .unwrap();
    let outcome_rl = CodexOAuthFetchStrategy::map_result_for_testing(
        &rate_limit_wins,
        &creds,
        None,
        false,
        true,
        now,
    )
    .unwrap();
    let limit_rl = outcome_rl
        .credits
        .as_ref()
        .unwrap()
        .codex_credit_limit
        .as_ref()
        .unwrap();
    assert_eq!(limit_rl.limit, 100_000.0);
    assert_eq!(limit_rl.remaining(), 92_239.0);
    assert_eq!(limit_rl.remaining_percent, 92.239);

    // 3. Spend-control fallback when root & rate_limit omit individual_limit
    let spend_control_only = serde_json::to_vec(&json!({
        "plan_type": "enterprise",
        "spendControl": {
            "individualLimit": {
                "limit": "1000",
                "remaining_percent": "40"
            }
        }
    }))
    .unwrap();
    let outcome2 = CodexOAuthFetchStrategy::map_result_for_testing(
        &spend_control_only,
        &creds,
        None,
        false,
        true,
        now,
    )
    .unwrap();
    let limit2 = outcome2
        .credits
        .as_ref()
        .unwrap()
        .codex_credit_limit
        .as_ref()
        .unwrap();
    assert_eq!(limit2.limit, 1000.0);
    assert_eq!(limit2.used, 600.0);
    assert_eq!(limit2.remaining_percent, 40.0);
}

#[tokio::test]
async fn replaces_zero_credit_limit_with_cli_monthly_limit_only_when_compatible() {
    let now = "2026-04-18T12:00:00Z".parse::<Timestamp>().unwrap();
    let id_token = make_jwt(&json!({
        "email": "Shared.User@Example.test"
    }));
    let creds = make_credentials_with_id_token(Some(id_token));
    let raw = serde_json::to_vec(&json!({
        "plan_type": "team",
        "rate_limit": {
            "primary_window": {
                "used_percent": 10,
                "reset_at": 1_800_000_000,
                "limit_window_seconds": 18_000
            }
        },
        "credits": {
            "has_credits": true,
            "unlimited": false,
            "balance": 0
        }
    }))
    .unwrap();

    let oauth_result =
        CodexOAuthFetchStrategy::map_result_for_testing(&raw, &creds, None, false, true, now)
            .unwrap();
    assert!(CodexOAuthFetchStrategy::should_try_cli_for_monthly_limit(
        &oauth_result
    ));

    let cli_limit =
        CodexCreditLimitSnapshot::new("Monthly credit limit", 80.0, 200.0, 60.0, None, now);
    let make_cli = |email: Option<&str>, limit: Option<CodexCreditLimitSnapshot>| {
        let cli_credits =
            CreditsSnapshot::with_details(0.0, Vec::new(), now, limit, true, Some(true), false);
        let cli_identity = email.map(|e| {
            ProviderIdentitySnapshot::new(
                Some(ProviderId::new("codex").unwrap()),
                Some(e.to_owned()),
                None,
                Some("team".to_owned()),
                None,
            )
        });
        let cli_usage = CodexExtraUsageCost::attaching(
            UsageSnapshot::new(
                None,
                None,
                None,
                None,
                None,
                Vec::new(),
                None,
                None,
                now,
                cli_identity,
                Confidence::Unknown,
            )
            .unwrap(),
            Some(&cli_credits),
        );
        CodexFetchOutcome {
            usage: cli_usage,
            credits: Some(cli_credits),
            codex_reset_credits: None,
            source_label: "codex-cli".to_owned(),
            strategy_id: CODEX_CLI_STRATEGY_ID,
            strategy_kind: SourceKind::Cli,
            codex_reset_credits_attempted: false,
            codex_monthly_limit_enrichment_failed: false,
        }
    };

    // 1. Matching email case-insensitively merges CLI monthly limit
    let cli_ok = make_cli(Some(" shared.user@EXAMPLE.test "), Some(cli_limit.clone()));
    let merged = CodexOAuthFetchStrategy::replacing_with_cli_monthly_limit_if_available(
        oauth_result.clone(),
        SourceMode::Auto,
        None,
        true,
        true,
        || async move { Ok(cli_ok) },
    )
    .await;
    assert_eq!(
        merged.credits.as_ref().unwrap().codex_credit_limit.as_ref(),
        Some(&cli_limit)
    );
    assert_eq!(merged.usage.provider_cost.as_ref().unwrap().limit, 200.0);

    // 2. include_credits = false does not launch CLI
    let cli_skip1 = make_cli(Some("shared.user@example.test"), Some(cli_limit.clone()));
    let unmerged1 = CodexOAuthFetchStrategy::replacing_with_cli_monthly_limit_if_available(
        oauth_result.clone(),
        SourceMode::Auto,
        None,
        false,
        true,
        || async move { Ok(cli_skip1) },
    )
    .await;
    assert!(
        unmerged1
            .credits
            .as_ref()
            .unwrap()
            .codex_credit_limit
            .is_none()
    );

    // 3. managed_workspace_account_id skips unscoped CLI monthly limit
    let cli_skip2 = make_cli(Some("shared.user@example.test"), Some(cli_limit.clone()));
    let unmerged2 = CodexOAuthFetchStrategy::replacing_with_cli_monthly_limit_if_available(
        oauth_result.clone(),
        SourceMode::Auto,
        Some("workspace-team"),
        true,
        true,
        || async move { Ok(cli_skip2) },
    )
    .await;
    assert!(
        unmerged2
            .credits
            .as_ref()
            .unwrap()
            .codex_credit_limit
            .is_none()
    );

    // 4. Missing CLI identity is rejected
    let cli_no_id = make_cli(None, Some(cli_limit.clone()));
    let unmerged3 = CodexOAuthFetchStrategy::replacing_with_cli_monthly_limit_if_available(
        oauth_result.clone(),
        SourceMode::Auto,
        None,
        true,
        true,
        || async move { Ok(cli_no_id) },
    )
    .await;
    assert!(
        unmerged3
            .credits
            .as_ref()
            .unwrap()
            .codex_credit_limit
            .is_none()
    );

    // 5. Different CLI account email is rejected
    let cli_other = make_cli(Some("other@example.test"), Some(cli_limit.clone()));
    let unmerged4 = CodexOAuthFetchStrategy::replacing_with_cli_monthly_limit_if_available(
        oauth_result.clone(),
        SourceMode::Auto,
        None,
        true,
        true,
        || async move { Ok(cli_other) },
    )
    .await;
    assert!(
        unmerged4
            .credits
            .as_ref()
            .unwrap()
            .codex_credit_limit
            .is_none()
    );

    // 6. Unavailable CLI or CLI without monthly limit keeps partial OAuth result
    let cli_no_limit = make_cli(Some("shared.user@example.test"), None);
    let unmerged5 = CodexOAuthFetchStrategy::replacing_with_cli_monthly_limit_if_available(
        oauth_result,
        SourceMode::Auto,
        None,
        true,
        true,
        || async move { Ok(cli_no_limit) },
    )
    .await;
    assert!(
        unmerged5
            .credits
            .as_ref()
            .unwrap()
            .codex_credit_limit
            .is_none()
    );
}

// ============================================================================
// CodexAdditionalRateLimitsTests.swift
// ============================================================================

#[test]
fn additional_rate_limits_maps_spark_and_non_spark_windows_and_dedupes_and_tolerates_non_bool_flags()
 {
    let now = "2026-04-18T12:00:00Z".parse::<Timestamp>().unwrap();
    let creds = make_credentials_with_id_token(None);

    let raw = serde_json::to_vec(&json!({
        "plan_type": "pro",
        "rate_limit": {
            "primary_window": {
                "used_percent": 10,
                "reset_at": 1_800_000_000,
                "limit_window_seconds": 18_000
            }
        },
        "additional_rate_limits": [
            {
                "limit_name": "GPT-5.3-Codex-Spark",
                "metered_feature": "codex_spark",
                "rate_limit": {
                    "allowed": "non-bool-tolerated",
                    "limit_reached": 0,
                    "primary_window": {
                        "used_percent": 35,
                        "reset_at": 1_800_000_500,
                        "limit_window_seconds": 18_000
                    },
                    "secondary_window": {
                        "used_percent": 70,
                        "reset_at": 1_800_500_000,
                        "limit_window_seconds": 604_800
                    }
                }
            },
            {
                "limit_name": "Code Review",
                "metered_feature": "code_review_sessions",
                "rate_limit": {
                    "primary_window": {
                        "used_percent": 50,
                        "reset_at": 1_800_001_000,
                        "limit_window_seconds": 18_000
                    }
                }
            },
            "malformed-non-object-entry"
        ]
    }))
    .unwrap();

    let outcome =
        CodexOAuthFetchStrategy::map_result_for_testing(&raw, &creds, None, false, true, now)
            .unwrap();
    assert_eq!(outcome.usage.data_confidence, Confidence::Unknown);

    let extras = outcome.usage.extra_rate_windows.as_ref().unwrap();
    assert_eq!(extras.len(), 3);
    assert_eq!(
        extras[0].id,
        CodexAdditionalRateLimitMapper::SPARK_PRIMARY_WINDOW_ID
    );
    assert_eq!(extras[0].title, "Codex Spark 5-hour");
    assert_eq!(extras[0].window.used_percent, 35.0);

    assert_eq!(
        extras[1].id,
        CodexAdditionalRateLimitMapper::SPARK_WEEKLY_WINDOW_ID
    );
    assert_eq!(extras[1].title, "Codex Spark Weekly");
    assert_eq!(extras[1].window.used_percent, 70.0);

    assert_eq!(extras[2].id, "codex-code-review-sessions");
    assert_eq!(extras[2].title, "Code Review");
    assert_eq!(extras[2].window.used_percent, 50.0);

    // Dedupes split Spark entries by window kind and duplicate non-Spark entries by slugged ID
    let entries: Vec<AdditionalRateLimit> = serde_json::from_value(json!([
        {
            "limit_name": "GPT-5.3-Codex-Spark",
            "metered_feature": "gpt_5_3_codex_spark",
            "rate_limit": {
                "primary_window": { "used_percent": 20, "reset_at": 1_766_948_068, "limit_window_seconds": 18_000 }
            }
        },
        {
            "limit_name": "GPT-5.3-Codex-Spark Weekly",
            "metered_feature": "gpt_5_3_codex_spark",
            "rate_limit": {
                "primary_window": { "used_percent": 80, "reset_at": 1_767_407_914, "limit_window_seconds": 604_800 }
            }
        },
        {
            "limit_name": "GPT-5.3-Codex-Spark Duplicate",
            "metered_feature": "gpt_5_3_codex_spark",
            "rate_limit": {
                "primary_window": { "used_percent": 99, "reset_at": 1_766_948_068, "limit_window_seconds": 18_000 }
            }
        },
        {
            "limit_name": "GPT-5.3-Codex-Mini",
            "metered_feature": "gpt_5_3_codex_mini",
            "rate_limit": {
                "primary_window": { "used_percent": 12, "reset_at": 1_766_948_068, "limit_window_seconds": 18_000 }
            }
        },
        {
            "limit_name": "GPT-5.3-Codex-Mini",
            "metered_feature": "gpt_5_3_codex_mini",
            "rate_limit": {
                "primary_window": { "used_percent": 99, "reset_at": 1_766_948_068, "limit_window_seconds": 18_000 }
            }
        }
    ]))
    .unwrap();
    let deduped = CodexAdditionalRateLimitMapper::extra_rate_windows(Some(&entries), now);
    assert_eq!(deduped.len(), 3);
    assert_eq!(deduped[0].id, "codex-spark");
    assert_eq!(deduped[0].window.used_percent, 20.0);
    assert_eq!(deduped[1].id, "codex-spark-weekly");
    assert_eq!(deduped[1].window.used_percent, 80.0);
    assert_eq!(deduped[2].id, "codex-gpt-5-3-codex-mini");
    assert_eq!(deduped[2].window.used_percent, 12.0);
}

// ============================================================================
// CodexExtraUsageCostTests.swift & CodexUnlimitedCreditsTests.swift
// ============================================================================

#[test]
fn extra_usage_cost_full_matrix_matches_upstream() {
    let now = Timestamp::from_second(1_780_000_000).unwrap();
    let reset_date = Timestamp::from_second(1_788_220_800).unwrap();

    // 1. Clamping on CodexCreditLimitSnapshot::new
    let clamped = CodexCreditLimitSnapshot::new("Cap", -5.0, -10.0, 140.0, None, now);
    assert_eq!(clamped.used, 0.0);
    assert_eq!(clamped.limit, 0.0);
    assert_eq!(clamped.remaining_percent, 100.0);

    // 2. Monthly credit maps to extra usage used vs limit, remaining == 0 -> balance == None
    let monthly_credits = CreditsSnapshot::new(0.0, Vec::new(), now).with_credit_limit(Some(
        CodexCreditLimitSnapshot::new(
            "Monthly credit limit",
            36.8,
            1000.0,
            96.32,
            Some(reset_date),
            now,
        ),
    ));
    let cost1 = CodexExtraUsageCost::provider_cost(Some(&monthly_credits)).unwrap();
    assert_eq!(cost1.used, 36.8);
    assert_eq!(cost1.limit, 1000.0);
    assert_eq!(cost1.currency_code, CodexExtraUsageCost::CURRENCY_CODE);
    assert_eq!(cost1.period.as_deref(), Some("Monthly credit limit"));
    assert_eq!(cost1.balance, None);
    assert_eq!(cost1.resets_at, Some(reset_date));

    // 3. Purchased extra credits are a remaining balance
    let purchased_only = CreditsSnapshot::new(14.5, Vec::new(), now);
    let cost2 = CodexExtraUsageCost::provider_cost(Some(&purchased_only)).unwrap();
    assert_eq!(cost2.used, 0.0);
    assert_eq!(cost2.limit, 0.0);
    assert_eq!(cost2.balance, Some(14.5));
    assert_eq!(cost2.period.as_deref(), Some("Extra usage"));

    // 4. Purchased extra credits sit beside a monthly cap (50 != 400 - 120 = 280)
    let cap_400 =
        CodexCreditLimitSnapshot::new("Monthly credit limit", 120.0, 400.0, 70.0, None, now);
    let beside_cap =
        CreditsSnapshot::new(50.0, Vec::new(), now).with_credit_limit(Some(cap_400.clone()));
    let cost3 = CodexExtraUsageCost::provider_cost(Some(&beside_cap)).unwrap();
    assert_eq!(cost3.used, 120.0);
    assert_eq!(cost3.limit, 400.0);
    assert_eq!(cost3.balance, Some(50.0));

    // 5. Monthly remaining (280 == 400 - 120) is NOT treated as purchased extra credits unless workspace balance
    let same_as_cap =
        CreditsSnapshot::new(280.0, Vec::new(), now).with_credit_limit(Some(cap_400.clone()));
    let cost4 = CodexExtraUsageCost::provider_cost(Some(&same_as_cap)).unwrap();
    assert_eq!(cost4.balance, None);
    // But a workspace balance equal to cap remainder IS preserved:
    let ws_same_as_cap = same_as_cap.with_balance_is_workspace(true);
    assert!(ws_same_as_cap.has_workspace_balance());
    assert_eq!(
        CodexExtraUsageCost::provider_cost(Some(&ws_same_as_cap))
            .unwrap()
            .balance,
        Some(280.0)
    );
    // And a purchased balance within 0.001 of cap remainder (above 0.000_1) IS preserved:
    let near_cap = CreditsSnapshot::new(280.001, Vec::new(), now).with_credit_limit(Some(cap_400));
    assert_eq!(
        CodexExtraUsageCost::provider_cost(Some(&near_cap))
            .unwrap()
            .balance,
        Some(280.001)
    );

    // 6. Zero remaining without a monthly cap retains the successful reading
    let zero_only = CreditsSnapshot::new(0.0, Vec::new(), now);
    let cost_zero = CodexExtraUsageCost::provider_cost(Some(&zero_only)).unwrap();
    assert_eq!(cost_zero.balance_updated_at, Some(now));
    assert_eq!(cost_zero.balance, None);
    assert!(CodexExtraUsageCost::provider_cost(None).is_none());

    // 7. Attached monthly cap wins over balance-only live credits
    let attached_cap = ProviderCostSnapshot {
        used: 120.0,
        limit: 400.0,
        currency_code: CodexExtraUsageCost::CURRENCY_CODE.to_owned(),
        period: Some("Monthly credit limit".to_owned()),
        resets_at: Some(reset_date),
        updated_at: now,
        next_regen_amount: None,
        personal_used: None,
        balance: None,
        balance_updated_at: None,
        balance_is_workspace: None,
        balance_is_unavailable: None,
    };
    let resolved1 =
        CodexExtraUsageCost::resolving(Some(&purchased_only), Some(&attached_cap)).unwrap();
    assert_eq!(resolved1.used, 120.0);
    assert_eq!(resolved1.limit, 400.0);
    assert_eq!(resolved1.resets_at, Some(reset_date));
    assert_eq!(resolved1.balance, Some(14.5));

    // 8. The fresher cap wins when both sides carry one, and the purchased balance comes from the fresher balance side
    let cap_fetched_at = now;
    let preserved = CreditsSnapshot::new(
        50.0,
        Vec::new(),
        cap_fetched_at
            .checked_add(Duration::from_secs(7200))
            .unwrap(),
    )
    .with_credit_limit(Some(CodexCreditLimitSnapshot::new(
        "Monthly credit limit",
        300.0,
        400.0,
        25.0,
        None,
        cap_fetched_at,
    )));
    let live_cost = CodexExtraUsageCost::provider_cost(Some(&preserved)).unwrap();
    assert_eq!(live_cost.updated_at, cap_fetched_at);

    let dashboard = ProviderCostSnapshot {
        used: 380.0,
        limit: 400.0,
        currency_code: CodexExtraUsageCost::CURRENCY_CODE.to_owned(),
        period: Some("Monthly credit limit".to_owned()),
        resets_at: None,
        updated_at: cap_fetched_at
            .checked_add(Duration::from_secs(3600))
            .unwrap(),
        next_regen_amount: None,
        personal_used: None,
        balance: Some(30.0),
        balance_updated_at: None,
        balance_is_workspace: None,
        balance_is_unavailable: None,
    };
    let resolved2 = CodexExtraUsageCost::resolving(Some(&preserved), Some(&dashboard)).unwrap();
    assert_eq!(resolved2.used, 380.0);
    assert_eq!(resolved2.balance, Some(50.0));

    let fresh_dash_bal = CodexExtraUsageCost::resolving(
        Some(&CreditsSnapshot::new(14.5, Vec::new(), cap_fetched_at)),
        Some(&dashboard),
    )
    .unwrap();
    assert_eq!(fresh_dash_bal.balance, Some(30.0));

    // 9. A winning live cap takes the attached balance when its own credits fetch failed (placeholder)
    let placeholder = CreditsSnapshot::with_details(
        0.0,
        Vec::new(),
        now,
        Some(CodexCreditLimitSnapshot::new(
            "Monthly credit limit",
            300.0,
            400.0,
            25.0,
            None,
            now,
        )),
        false,
        None,
        false,
    );
    let older_dashboard = ProviderCostSnapshot {
        used: 120.0,
        limit: 400.0,
        currency_code: CodexExtraUsageCost::CURRENCY_CODE.to_owned(),
        period: Some("Monthly credit limit".to_owned()),
        resets_at: None,
        updated_at: now.checked_sub(Duration::from_secs(3600)).unwrap(),
        next_regen_amount: None,
        personal_used: None,
        balance: Some(30.0),
        balance_updated_at: None,
        balance_is_workspace: None,
        balance_is_unavailable: None,
    };
    let resolved3 =
        CodexExtraUsageCost::resolving(Some(&placeholder), Some(&older_dashboard)).unwrap();
    assert_eq!(resolved3.used, 300.0);
    assert_eq!(resolved3.balance, Some(30.0));

    // 10. A newer successful zero purchased-credit balance clears an older attached balance
    let confirmed_zero = placeholder.clone().with_balance_read_succeeded(true);
    let resolved4 =
        CodexExtraUsageCost::resolving(Some(&confirmed_zero), Some(&older_dashboard)).unwrap();
    assert_eq!(resolved4.used, 300.0);
    assert_eq!(resolved4.balance, None);

    // 11. Legacy CreditsSnapshot JSON without balanceReadSucceeded defaults to true
    let legacy_json = br#"{"remaining":14.5,"events":[],"updatedAt":"2026-05-28T21:20:00Z"}"#;
    let legacy_decoded: CreditsSnapshot = serde_json::from_slice(legacy_json).unwrap();
    assert_eq!(legacy_decoded.remaining, 14.5);
    assert!(legacy_decoded.balance_read_succeeded);

    // 12. Resolving keeps each side when the other is missing or foreign (USD)
    let foreign = ProviderCostSnapshot {
        used: 4.0,
        limit: 50.0,
        currency_code: "USD".to_owned(),
        period: None,
        resets_at: None,
        updated_at: now,
        next_regen_amount: None,
        personal_used: None,
        balance: None,
        balance_updated_at: None,
        balance_is_workspace: None,
        balance_is_unavailable: None,
    };
    assert_eq!(
        CodexExtraUsageCost::resolving(None, Some(&foreign)),
        Some(foreign.clone())
    );
    assert_eq!(CodexExtraUsageCost::resolving(None, None), None);
    assert_eq!(
        CodexExtraUsageCost::resolving(Some(&purchased_only), None),
        Some(cost2.clone())
    );
    assert_eq!(
        CodexExtraUsageCost::resolving(Some(&purchased_only), Some(&foreign)),
        Some(cost2)
    );
}

#[test]
fn unlimited_credits_matrix_across_oauth_and_pat_routes() {
    let now = Timestamp::from_second(1_700_000_000).unwrap();
    let creds = make_credentials_with_id_token(None);
    let whoami = CodexPATWhoami {
        account_id: None,
        email: Some("fixture@example.com".to_owned()),
        plan_type: Some("business".to_owned()),
    };

    let make_usage_data =
        |has_credits: bool, unlimited: bool, balance: Option<f64>, has_cap: bool| -> Vec<u8> {
            let mut obj = json!({
                "plan_type": "business",
                "rate_limit": {
                    "primary_window": {
                        "used_percent": 12,
                        "reset_at": 1_700_003_600,
                        "limit_window_seconds": 18_000
                    }
                },
                "credits": {
                    "has_credits": has_credits,
                    "unlimited": unlimited
                }
            });
            if let Some(b) = balance {
                obj["credits"]["balance"] = json!(b);
            }
            if has_cap {
                obj["individual_limit"] = json!({"limit": 400, "used": 300});
            }
            serde_json::to_vec(&obj).unwrap()
        };

    // 1. Unlimited credits without a finite balance or cap do not create unavailable credits
    for route in ["oauth", "pat"] {
        for has_credits in [false, true] {
            let data = make_usage_data(has_credits, true, None, false);
            let res = if route == "oauth" {
                CodexOAuthFetchStrategy::map_result_for_testing(
                    &data, &creds, None, false, true, now,
                )
                .unwrap()
            } else {
                CodexPATFetchStrategy::map_result_for_testing(&data, Some(&whoami), now).unwrap()
            };
            assert_eq!(res.usage.primary.as_ref().unwrap().used_percent, 12.0);
            assert!(res.credits.is_none());
            assert!(res.usage.provider_cost.is_none());
        }
    }

    // 2. Unlimited flag preserves independently reported finite balances and monthly caps
    for route in ["oauth", "pat"] {
        for (balance, has_cap) in [(Some(14.0), false), (Some(0.0), false), (None, true)] {
            let data = make_usage_data(true, true, balance, has_cap);
            let res = if route == "oauth" {
                CodexOAuthFetchStrategy::map_result_for_testing(
                    &data, &creds, None, false, true, now,
                )
                .unwrap()
            } else {
                CodexPATFetchStrategy::map_result_for_testing(&data, Some(&whoami), now).unwrap()
            };
            let credits = res.credits.as_ref().unwrap();
            let cost = res.usage.provider_cost.as_ref().unwrap();
            assert_eq!(credits.remaining, balance.unwrap_or(0.0));
            assert_eq!(credits.balance_read_succeeded, balance.is_some());
            assert_eq!(
                credits.display_remaining(),
                if has_cap { Some(100.0) } else { balance }
            );
            assert_ne!(cost.balance_is_unavailable, Some(true));
            assert_eq!(
                cost.balance,
                if balance == Some(14.0) {
                    Some(14.0)
                } else {
                    None
                }
            );
            assert_eq!(
                cost.balance_updated_at,
                if balance.is_some() {
                    Some(credits.updated_at)
                } else {
                    None
                }
            );
            assert_eq!(cost.limit, if has_cap { 400.0 } else { 0.0 });
        }
    }

    // 3. Limited available credits with a missing balance still create an unavailable observation
    for route in ["oauth", "pat"] {
        let data = make_usage_data(true, false, None, false);
        let res = if route == "oauth" {
            CodexOAuthFetchStrategy::map_result_for_testing(&data, &creds, None, false, true, now)
                .unwrap()
        } else {
            CodexPATFetchStrategy::map_result_for_testing(&data, Some(&whoami), now).unwrap()
        };
        let credits = res.credits.as_ref().unwrap();
        let cost = res.usage.provider_cost.as_ref().unwrap();
        assert!(!credits.balance_read_succeeded);
        assert_eq!(credits.credits_available, Some(true));
        assert_eq!(credits.display_remaining(), None);
        assert_eq!(cost.balance_is_unavailable, Some(true));
        assert_eq!(cost.balance_updated_at, Some(credits.updated_at));
        assert_eq!(cost.balance, None);
    }
}

// ============================================================================
// CodexIdentityResolverTests.swift & CodexPlanFormattingTests.swift
// ============================================================================

#[test]
fn identity_resolver_plan_formatting_and_redacted_debug_match_upstream() {
    assert_eq!(
        CodexIdentityResolver::resolve(Some("account-123"), Some("Person@example.com")),
        CodexIdentity::ProviderAccount {
            id: "account-123".to_owned()
        }
    );
    assert_eq!(
        CodexIdentityResolver::resolve(None, Some(" Person@example.com ")),
        CodexIdentity::EmailOnly {
            normalized_email: "person@example.com".to_owned()
        }
    );
    // Unicode lowercase normalization
    assert_eq!(
        CodexIdentityResolver::normalize_email(Some(" Üser@Example.test ")).as_deref(),
        Some("üser@example.test")
    );
    assert_eq!(
        CodexIdentityResolver::resolve(Some("   "), Some("")),
        CodexIdentity::Unresolved
    );
    assert_ne!(
        CodexIdentityResolver::resolve(Some("account-123"), Some("person@example.com")),
        CodexIdentityResolver::resolve(None, Some("person@example.com"))
    );

    // Exact case-sensitive account_id match, case-insensitive email match when account_id absent
    let owner = CodexNativeCredentialOwnerIdentity::new(Some("Acct-1"), Some("User@Example.test"));
    let same_acct =
        CodexNativeCredentialOwnerIdentity::new(Some("Acct-1"), Some("different@example.test"));
    let diff_case_acct =
        CodexNativeCredentialOwnerIdentity::new(Some("acct-1"), Some("User@Example.test"));
    assert!(owner.matches(&same_acct));
    assert!(!owner.matches(&diff_case_acct));

    // JWT id_token email extraction checks top-level vs profile agreement
    let agree_jwt = make_jwt(&json!({
        "email": "User@Example.test",
        "https://api.openai.com/profile": { "email": "user@example.test" }
    }));
    assert_eq!(
        CodexNativeCredentialOwnerIdentity::normalized_email_from_id_token(Some(&agree_jwt))
            .as_deref(),
        Some("user@example.test")
    );
    let disagree_jwt = make_jwt(&json!({
        "email": "user1@example.test",
        "https://api.openai.com/profile": { "email": "user2@example.test" }
    }));
    assert_eq!(
        CodexNativeCredentialOwnerIdentity::normalized_email_from_id_token(Some(&disagree_jwt)),
        None
    );

    // Debug impls redact emails and account IDs
    let id_debug = format!("{:?}", owner);
    assert!(!id_debug.contains("Acct-1"));
    assert!(!id_debug.contains("User@Example.test"));
    assert!(!id_debug.contains("user@example.test"));
    let whoami_debug = format!(
        "{:?}",
        CodexPATWhoami {
            account_id: Some("secret-acct-123".to_owned()),
            email: Some("secret-email@example.test".to_owned()),
            plan_type: Some("pro".to_owned()),
        }
    );
    assert!(!whoami_debug.contains("secret-acct-123"));
    assert!(!whoami_debug.contains("secret-email@example.test"));

    // Plan formatting parity with CodexPlanFormattingTests.swift
    for pro_variant in ["pro", "Pro", "Codex Pro"] {
        assert_eq!(
            CodexPlanFormatting::display_name(pro_variant).as_deref(),
            Some("Pro 20x")
        );
    }
    for prolite_variant in [
        "prolite",
        "pro_lite",
        "pro-lite",
        "Pro Lite",
        "Codex Pro Lite",
    ] {
        assert_eq!(
            CodexPlanFormatting::display_name(prolite_variant).as_deref(),
            Some("Pro 5x")
        );
    }
    assert_eq!(CodexPlanFormatting::display_name(""), None);
    assert_eq!(CodexPlanFormatting::display_name("   "), None);
    assert_eq!(
        CodexPlanFormatting::display_name("enterprise_cbp_usage_based").as_deref(),
        Some("Enterprise CBP Usage Based")
    );
    assert_eq!(
        CodexPlanFormatting::display_name("self_serve_business_usage_based").as_deref(),
        Some("Self Serve Business Usage Based")
    );
    assert_eq!(
        CodexPlanFormatting::display_name("k12").as_deref(),
        Some("K12")
    );
    assert_eq!(
        CodexPlanFormatting::display_name("Enterprise").as_deref(),
        Some("Enterprise")
    );
    assert_eq!(
        CodexPlanFormatting::display_name("plus plan").as_deref(),
        Some("Plus")
    );
}

// ============================================================================
// CodexRateLimitResetCreditsTests.swift & CodexOAuthResetCreditFetchTests.swift
// ============================================================================

#[test]
fn rate_limit_reset_credits_hash_id_sort_inventory_and_preserve_canonical_ids() {
    let now = "2026-07-01T00:00:00Z".parse::<Timestamp>().unwrap();
    let raw_json = serde_json::to_vec(&json!({
        "available_count": 2,
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
                "expires_at": "2026-07-18T00:39:53.731630Z"
            },
            {
                "id": "RateLimitResetCredit_earlier",
                "reset_type": "codex_rate_limits",
                "status": "available",
                "granted_at": "2026-06-12T04:03:43.263391Z",
                "expires_at": "2026-07-12T04:03:43.263391Z"
            },
            {
                "id": "RateLimitResetCredit_redeeming",
                "reset_type": "codex_rate_limits",
                "status": "redeeming",
                "granted_at": "2026-06-12T04:03:43Z",
                "expires_at": "2026-07-10T04:03:43Z"
            },
            {
                "id": "RateLimitResetCredit_future_status",
                "reset_type": "codex_rate_limits",
                "status": "future_status",
                "granted_at": "2026-06-12T04:03:43Z",
                "expires_at": "2026-07-10T04:03:43Z"
            }
        ]
    }))
    .unwrap();

    let snap = CodexOAuthUsageFetcher::decode_rate_limit_reset_credits(&raw_json, now).unwrap();
    assert_eq!(snap.credits.len(), 5);
    assert_eq!(
        snap.credits[3].status,
        CodexRateLimitResetCreditStatus::Redeeming
    );
    assert_eq!(
        snap.credits[4].status,
        CodexRateLimitResetCreditStatus::Unknown("future_status".to_owned())
    );
    assert_eq!(
        CodexRateLimitResetCreditStatus::from_raw("AVAILABLE"),
        CodexRateLimitResetCreditStatus::Unknown("AVAILABLE".to_owned())
    );
    assert!(
        snap.credits
            .iter()
            .all(|c| !c.id.contains("RateLimitResetCredit_"))
    );

    let inv = snap.available_inventory(now);
    assert_eq!(inv.count(), 2);
    assert_eq!(
        inv.credits[0].id,
        CodexRateLimitResetCredit::stable_id_for_provider_id("RateLimitResetCredit_earlier")
    );

    // Provider IDs always hash even when shaped like persisted stable IDs
    let canonical_looking = format!("codex-reset-credit-v1-{}", "a".repeat(64));
    let expected_hashed = CodexRateLimitResetCredit::stable_id_for_provider_id(&canonical_looking);
    assert_ne!(expected_hashed, canonical_looking);
    let model = CodexRateLimitResetCredit::new(
        &canonical_looking,
        "codex_rate_limits",
        CodexRateLimitResetCreditStatus::Available,
        now,
        None,
        None,
        None,
        None,
        None,
    );
    assert_eq!(model.id, expected_hashed);
    let encoded = serde_json::to_vec(&model).unwrap();
    let round_tripped: CodexRateLimitResetCredit = serde_json::from_slice(&encoded).unwrap();
    assert_eq!(round_tripped.id, expected_hashed);

    // Available inventory keeps no-expiry credits and sorts deterministically
    let tied_expiry = now.checked_add(Duration::from_secs(3600)).unwrap();
    let mk_credit = |id: &str, status: CodexRateLimitResetCreditStatus, exp: Option<Timestamp>| {
        CodexRateLimitResetCredit::new(
            id,
            "codex_rate_limits",
            status,
            now,
            exp,
            None,
            None,
            None,
            None,
        )
    };
    let sort_snap = CodexRateLimitResetCreditsSnapshot::new(
        vec![
            mk_credit("nil-b", CodexRateLimitResetCreditStatus::Available, None),
            mk_credit(
                "expired",
                CodexRateLimitResetCreditStatus::Available,
                Some(now),
            ),
            mk_credit(
                "finite-b",
                CodexRateLimitResetCreditStatus::Available,
                Some(tied_expiry),
            ),
            mk_credit(
                "redeemed",
                CodexRateLimitResetCreditStatus::Redeemed,
                Some(now.checked_add(Duration::from_secs(7200)).unwrap()),
            ),
            mk_credit("nil-a", CodexRateLimitResetCreditStatus::Available, None),
            mk_credit(
                "finite-a",
                CodexRateLimitResetCreditStatus::Available,
                Some(tied_expiry),
            ),
        ],
        99,
        now,
    );
    let sort_inv = sort_snap.available_inventory(now);
    assert_eq!(sort_inv.count(), 4);
    let mut expected_finite: Vec<String> = ["finite-a", "finite-b"]
        .into_iter()
        .map(CodexRateLimitResetCredit::stable_id_for_provider_id)
        .collect();
    expected_finite.sort();
    let mut expected_nil: Vec<String> = ["nil-a", "nil-b"]
        .into_iter()
        .map(CodexRateLimitResetCredit::stable_id_for_provider_id)
        .collect();
    expected_nil.sort();
    let actual_ids: Vec<String> = sort_inv.credits.iter().map(|c| c.id.clone()).collect();
    let mut expected_all = expected_finite.clone();
    expected_all.extend(expected_nil);
    assert_eq!(actual_ids, expected_all);

    // Rejects negative available_count
    assert_eq!(
        CodexOAuthUsageFetcher::decode_rate_limit_reset_credits(
            br#"{"credits":[],"available_count":-1}"#,
            now
        )
        .unwrap_err(),
        CodexOAuthFetchError::InvalidResponse
    );
}

#[test]
fn should_fetch_reset_credits_respects_runtime_and_flag() {
    let mut cli_no_credits = make_fetch_context(SourceMode::Oauth);
    cli_no_credits.runtime = FetchRuntime::Cli;
    cli_no_credits.include_credits = false;
    assert!(!CodexOAuthFetchStrategy::should_fetch_reset_credits(
        &cli_no_credits
    ));

    let mut cli_with_credits = cli_no_credits.clone();
    cli_with_credits.include_credits = true;
    assert!(CodexOAuthFetchStrategy::should_fetch_reset_credits(
        &cli_with_credits
    ));

    let mut daemon_no_credits = cli_no_credits;
    daemon_no_credits.runtime = FetchRuntime::Daemon;
    daemon_no_credits.interaction = FetchInteraction::Background;
    assert!(CodexOAuthFetchStrategy::should_fetch_reset_credits(
        &daemon_no_credits
    ));
}

// ============================================================================
// CodexSpendControlsMonthlyUsageTests.swift & CodexWorkspaceBalanceTests.swift
// ============================================================================

#[tokio::test]
async fn spend_controls_monthly_usage_and_workspace_balance_enrichment() {
    let now = "2026-04-18T12:00:00Z".parse::<Timestamp>().unwrap();
    let creds = make_credentials_with_id_token(None);

    let raw_usage = serde_json::to_vec(&json!({
        "account_id": "acct-test-123",
        "plan_type": "education",
        "spend_control": { "reached": false, "individual_limit": null },
        "rate_limit": {
            "primary_window": {
                "used_percent": 10,
                "reset_at": 1_800_000_000,
                "limit_window_seconds": 18_000
            }
        },
        "credits": {
            "has_credits": true,
            "unlimited": false
        }
    }))
    .unwrap();
    let usage_resp = CodexOAuthUsageFetcher::decode_usage_response(&raw_usage).unwrap();
    assert_eq!(usage_resp.account_id.as_deref(), Some("acct-test-123"));
    assert!(usage_resp.resolved_individual_limit().is_none());
    assert!(usage_resp.spend_control_present);
    assert!(CodexSpendControlsMonthlyUsageGate::should_fetch(
        &usage_resp
    ));

    let initial =
        CodexOAuthFetchStrategy::map_result_for_testing(&raw_usage, &creds, None, false, true, now)
            .unwrap();

    // 1. Apply workspace remaining balance
    let with_ws = CodexOAuthFetchStrategy::applying_workspace_remaining_balance(
        initial.clone(),
        &usage_resp,
        &creds,
        true,
        now,
        |acct_id| async move {
            assert_eq!(acct_id, "acct-test-123");
            CodexOAuthUsageFetcher::decode_workspace_remaining_balance_response(
                br#"{"balance":"150.25"}"#,
            )
        },
    )
    .await;
    let ws_credits = with_ws.credits.as_ref().unwrap();
    assert_eq!(ws_credits.remaining, 150.25);
    assert!(ws_credits.balance_is_workspace);
    assert!(ws_credits.balance_read_succeeded);

    // 2. Apply spend-controls monthly limit
    let with_sc = CodexOAuthFetchStrategy::applying_spend_controls_monthly_limit(
        with_ws,
        &usage_resp,
        &creds,
        true,
        |_acct_id| async move {
            CodexOAuthUsageFetcher::decode_spend_controls_monthly_usage_response(
                br#"{"current_month_usage":3046.4506806135178,"effective_monthly_limit":{"limit":7000,"enforcement_mode":"HARD_CAP","limit_mode":"amount_credits"}}"#,
            )
        },
    )
    .await;
    assert!(!with_sc.codex_monthly_limit_enrichment_failed);
    let sc_credits = with_sc.credits.as_ref().unwrap();
    let limit = sc_credits.codex_credit_limit.as_ref().unwrap();
    assert!((limit.used - 3046.4506806135178).abs() < 1e-6);
    assert_eq!(limit.limit, 7000.0);
    assert!((limit.remaining_percent - 56.4792760).abs() < 1e-6);

    // Project into Snapshot v1 ProviderSnapshot and verify fields
    let snapshot_v1 = with_sc.project_provider_snapshot(now);
    assert_eq!(snapshot_v1.id, ProviderId::new("codex").unwrap());
    let wire_credits = snapshot_v1
        .credits
        .as_ref()
        .unwrap()
        .value
        .as_ref()
        .unwrap();
    assert_eq!(wire_credits.remaining, 150.25);
    assert_eq!(wire_credits.balance_is_workspace, Some(true));
    assert_eq!(wire_credits.credit_limit.as_ref().unwrap().total, 7000.0);
    let wire_cost = snapshot_v1.cost.as_ref().unwrap().value.as_ref().unwrap();
    assert_eq!(wire_cost.limit, Some(7000.0));
    assert_eq!(wire_cost.balance, Some(150.25));

    // Enrichment failure marks codex_monthly_limit_enrichment_failed while keeping original credits
    let failed_sc = CodexOAuthFetchStrategy::applying_spend_controls_monthly_limit(
        initial.clone(),
        &usage_resp,
        &creds,
        true,
        |_acct_id| async move { Err(CodexOAuthFetchError::InvalidResponse) },
    )
    .await;
    assert!(failed_sc.codex_monthly_limit_enrichment_failed);
    assert_eq!(failed_sc.credits, initial.credits);

    // Missing account_id marks enrichment failure without invoking fetcher
    let no_acct_usage = CodexOAuthUsageFetcher::decode_usage_response(
        br#"{"plan_type":"education","spend_control":{}}"#,
    )
    .unwrap();
    let no_acct_creds = CodexOAuthCredentials::new(
        "tok",
        "ref",
        None,
        None,
        Some(now),
        None,
        CodexOAuthCredentialSource::CodexHome,
        false,
    );
    let calls = Arc::new(AtomicUsize::new(0));
    let calls_clone = Arc::clone(&calls);
    let missing_acct_res = CodexOAuthFetchStrategy::applying_spend_controls_monthly_limit(
        initial,
        &no_acct_usage,
        &no_acct_creds,
        true,
        move |_acct_id| {
            calls_clone.fetch_add(1, Ordering::SeqCst);
            async move { Err(CodexOAuthFetchError::InvalidResponse) }
        },
    )
    .await;
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert!(missing_acct_res.codex_monthly_limit_enrichment_failed);
}

#[test]
fn spend_controls_enforcement_modes_malformed_shapes_and_url_resolution() {
    let now = "2026-04-18T12:00:00Z".parse::<Timestamp>().unwrap();

    // Active and unknown enforcement modes produce a monthly limit
    for mode in [
        Some("HARD_CAP"),
        Some("SOFT_CAP"),
        Some("future_mode"),
        None,
    ] {
        let mode_json = mode
            .map(|m| format!(r#","enforcement_mode":"{m}""#))
            .unwrap_or_default();
        let raw = format!(
            r#"{{"current_month_usage":12,"effective_monthly_limit":{{"limit":7000{mode_json}}}}}"#
        );
        let resp: CodexSpendControlsMonthlyUsageResponse = serde_json::from_str(&raw).unwrap();
        assert!(resp.codex_credit_limit_snapshot(now).is_some());
        assert!(!resp.monthly_limit_mapping_failed());
    }

    // Inactive enforcement modes suppress the monthly limit case-insensitively
    for mode in ["NONE", "disabled", "OFF", "no_limit"] {
        let raw = format!(
            r#"{{"current_month_usage":12,"effective_monthly_limit":{{"limit":7000,"enforcement_mode":"{mode}"}}}}"#
        );
        let resp: CodexSpendControlsMonthlyUsageResponse = serde_json::from_str(&raw).unwrap();
        assert!(resp.codex_credit_limit_snapshot(now).is_none());
        assert!(!resp.monthly_limit_mapping_failed());
    }

    // Missing or zero limit suppresses snapshot without mapping failure
    for raw in [
        r#"{"current_month_usage":12,"effective_monthly_limit":{"limit":0,"enforcement_mode":"HARD_CAP"}}"#,
        r#"{"current_month_usage":12,"effective_monthly_limit":{"enforcement_mode":"HARD_CAP"}}"#,
    ] {
        let resp: CodexSpendControlsMonthlyUsageResponse = serde_json::from_str(raw).unwrap();
        assert!(resp.codex_credit_limit_snapshot(now).is_none());
        assert!(!resp.monthly_limit_mapping_failed());
    }

    // Malformed nested limit is a mapping failure unless enforcement is disabled
    let malformed_limit: CodexSpendControlsMonthlyUsageResponse = serde_json::from_str(
        r#"{"current_month_usage":12,"effective_monthly_limit":{"limit":{"amount":7000},"enforcement_mode":"HARD_CAP"}}"#,
    )
    .unwrap();
    assert!(malformed_limit.codex_credit_limit_snapshot(now).is_none());
    assert!(malformed_limit.monthly_limit_mapping_failed());

    let disabled_malformed_limit: CodexSpendControlsMonthlyUsageResponse = serde_json::from_str(
        r#"{"current_month_usage":12,"effective_monthly_limit":{"limit":{"amount":7000},"enforcement_mode":"disabled"}}"#,
    )
    .unwrap();
    assert!(
        disabled_malformed_limit
            .codex_credit_limit_snapshot(now)
            .is_none()
    );
    assert!(!disabled_malformed_limit.monthly_limit_mapping_failed());

    // Malformed current_month_usage is a mapping failure unless cap is absent
    let malformed_usage: CodexSpendControlsMonthlyUsageResponse = serde_json::from_str(
        r#"{"current_month_usage":{"amount":12},"effective_monthly_limit":{"limit":7000,"enforcement_mode":"HARD_CAP"}}"#,
    )
    .unwrap();
    assert!(malformed_usage.monthly_limit_mapping_failed());

    let absent_cap_malformed_usage: CodexSpendControlsMonthlyUsageResponse =
        serde_json::from_str(r#"{"current_month_usage":{"amount":12}}"#).unwrap();
    assert!(!absent_cap_malformed_usage.monthly_limit_mapping_failed());

    // Malformed enforcement_mode is a mapping failure unless limit is 0
    let malformed_enf: CodexSpendControlsMonthlyUsageResponse = serde_json::from_str(
        r#"{"current_month_usage":12,"effective_monthly_limit":{"limit":7000,"enforcement_mode":{"mode":"HARD_CAP"}}}"#,
    )
    .unwrap();
    assert!(malformed_enf.monthly_limit_mapping_failed());

    let zero_limit_malformed_enf: CodexSpendControlsMonthlyUsageResponse = serde_json::from_str(
        r#"{"current_month_usage":12,"effective_monthly_limit":{"limit":0,"enforcement_mode":{"mode":"HARD_CAP"}}}"#,
    )
    .unwrap();
    assert!(!zero_limit_malformed_enf.monthly_limit_mapping_failed());

    // Non-finite strings ("NaN", "Infinity") are mapping failures; negative usage clamps to 0
    let nan_usage: CodexSpendControlsMonthlyUsageResponse = serde_json::from_str(
        r#"{"current_month_usage":"NaN","effective_monthly_limit":{"limit":7000,"enforcement_mode":"HARD_CAP"}}"#,
    )
    .unwrap();
    assert!(nan_usage.monthly_limit_mapping_failed());

    let inf_limit: CodexSpendControlsMonthlyUsageResponse = serde_json::from_str(
        r#"{"current_month_usage":12,"effective_monthly_limit":{"limit":"Infinity","enforcement_mode":"HARD_CAP"}}"#,
    )
    .unwrap();
    assert!(inf_limit.codex_credit_limit_snapshot(now).is_none());
    assert!(inf_limit.monthly_limit_mapping_failed());

    let neg_usage: CodexSpendControlsMonthlyUsageResponse = serde_json::from_str(
        r#"{"current_month_usage":"-20","effective_monthly_limit":{"limit":"7000","enforcement_mode":"HARD_CAP"}}"#,
    )
    .unwrap();
    let neg_snap = neg_usage.codex_credit_limit_snapshot(now).unwrap();
    assert_eq!(neg_snap.used, 0.0);
    assert_eq!(neg_snap.remaining_percent, 100.0);

    // Consumer plans never gate the monthly endpoint; unknown/missing plans with spendControl do
    for plan in ["plus", "pro", "free"] {
        let u: CodexUsageResponse = serde_json::from_value(json!({
            "plan_type": plan,
            "spend_control": {}
        }))
        .unwrap();
        assert!(!CodexSpendControlsMonthlyUsageGate::should_fetch(&u));
    }
    let camel_u = CodexOAuthUsageFetcher::decode_usage_response(
        br#"{"accountId":"camel-id","plan_type":"edu","spendControl":null}"#,
    )
    .unwrap();
    assert_eq!(camel_u.account_id.as_deref(), Some("camel-id"));
    assert!(camel_u.spend_control_present);
    assert!(CodexSpendControlsMonthlyUsageGate::should_fetch(&camel_u));
    assert_eq!(camel_u.plan_type, Some(PlanType::Edu));

    // URL resolution requires /backend-api and percent-encodes reserved characters
    let empty_env = BTreeMap::new();
    assert_eq!(
        CodexOAuthUsageFetcher::resolve_spend_controls_monthly_usage_url(
            &empty_env,
            Some(r#"chatgpt_base_url = "https://chatgpt.com/backend-api""#),
            None,
            "acct/a b",
        )
        .unwrap()
        .as_str(),
        "https://chatgpt.com/backend-api/accounts/acct%2Fa%20b/spend-controls/current-user/monthly-usage"
    );
    assert!(
        CodexOAuthUsageFetcher::resolve_spend_controls_monthly_usage_url(
            &empty_env,
            Some(r#"chatgpt_base_url = "https://api.openai.com/api/codex""#),
            None,
            "acct-123",
        )
        .is_none()
    );
}

#[tokio::test]
async fn workspace_balance_display_resolution_and_enrichment_rules() {
    let now = "2026-04-18T12:00:00Z".parse::<Timestamp>().unwrap();
    let older = now.checked_sub(Duration::from_secs(60)).unwrap();

    // 1. Deferred balance refresh preserves workspace balance while requested missing read invalidates it
    for (include_credits, has_cap) in [(false, false), (false, true), (true, false), (true, true)] {
        let mut obj = json!({
            "account_id": "workspace-fixture",
            "plan_type": "business",
            "credits": { "has_credits": true, "unlimited": false, "balance": null }
        });
        if has_cap {
            obj["individual_limit"] = json!({"limit": 400, "used": 300});
        }
        let raw = serde_json::to_vec(&obj).unwrap();
        let creds = make_credentials_with_id_token(None);
        let result = CodexOAuthFetchStrategy::map_result_for_testing(
            &raw,
            &creds,
            None,
            false,
            include_credits,
            now,
        )
        .unwrap();
        let retained =
            CreditsSnapshot::new(1234.0, Vec::new(), older).with_balance_is_workspace(true);
        let display = CodexExtraUsageCost::credits_for_display(
            Some(&retained),
            result.usage.provider_cost.as_ref(),
        )
        .unwrap();
        let expected_rem = if include_credits {
            if has_cap { Some(100.0) } else { None }
        } else {
            Some(1234.0)
        };
        assert_eq!(display.display_remaining(), expected_rem);
        assert_eq!(display.balance_read_succeeded, !include_credits);
        assert_eq!(display.has_workspace_balance(), !include_credits);
    }

    // 2. Usage-only numeric balances remain authoritative over an older workspace balance
    for balance in [0.0, 14.0] {
        let raw = serde_json::to_vec(&json!({
            "account_id": "workspace-fixture",
            "plan_type": "business",
            "credits": { "has_credits": true, "unlimited": false, "balance": balance }
        }))
        .unwrap();
        let creds = make_credentials_with_id_token(None);
        let result =
            CodexOAuthFetchStrategy::map_result_for_testing(&raw, &creds, None, false, false, now)
                .unwrap();
        let retained =
            CreditsSnapshot::new(1234.0, Vec::new(), older).with_balance_is_workspace(true);
        let display = CodexExtraUsageCost::credits_for_display(
            Some(&retained),
            result.usage.provider_cost.as_ref(),
        )
        .unwrap();
        assert_eq!(display.remaining, balance);
        assert!(display.balance_read_succeeded);
        assert!(!display.has_workspace_balance());
    }

    // 3. Conflicting credential and response accounts must not fetch a workspace balance
    let raw_conflict = serde_json::to_vec(&json!({
        "account_id": "response-workspace",
        "plan_type": "business",
        "rate_limit": {
            "primary_window": { "used_percent": 10, "reset_at": 1_800_000_000, "limit_window_seconds": 18_000 }
        },
        "credits": { "has_credits": true, "unlimited": false, "balance": null }
    }))
    .unwrap();
    let usage_conflict = CodexOAuthUsageFetcher::decode_usage_response(&raw_conflict).unwrap();
    let creds_conflict = CodexOAuthCredentials::new(
        "tok",
        "ref",
        None,
        Some("different-workspace".to_owned()),
        Some(now),
        None,
        CodexOAuthCredentialSource::CodexHome,
        false,
    );
    let orig_conflict = CodexOAuthFetchStrategy::map_result_for_testing(
        &raw_conflict,
        &creds_conflict,
        None,
        false,
        true,
        now,
    )
    .unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let calls_clone = Arc::clone(&calls);
    let unchanged = CodexOAuthFetchStrategy::applying_workspace_remaining_balance(
        orig_conflict.clone(),
        &usage_conflict,
        &creds_conflict,
        true,
        now,
        move |_acct| {
            calls_clone.fetch_add(1, Ordering::SeqCst);
            async move { Err(CodexOAuthFetchError::InvalidResponse) }
        },
    )
    .await;
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert_eq!(unchanged.credits, orig_conflict.credits);

    // 4. Workspace URL encodes reserved account characters and rejects empty or non-backend-api bases
    let empty_env = BTreeMap::new();
    assert_eq!(
        CodexOAuthUsageFetcher::resolve_workspace_remaining_balance_url(
            &empty_env,
            Some(r#"chatgpt_base_url = "https://chatgpt.com/backend-api""#),
            None,
            "acct/a b?#%",
        )
        .unwrap()
        .as_str(),
        "https://chatgpt.com/backend-api/accounts/acct%2Fa%20b%3F%23%25/remaining_balance"
    );
    assert!(
        CodexOAuthUsageFetcher::resolve_workspace_remaining_balance_url(
            &empty_env,
            Some(r#"chatgpt_base_url = "https://api.openai.com/api/codex""#),
            None,
            "fixture-account",
        )
        .is_none()
    );
    assert!(
        CodexOAuthUsageFetcher::resolve_workspace_remaining_balance_url(
            &empty_env,
            Some(r#"chatgpt_base_url = "https://chatgpt.com""#),
            None,
            "",
        )
        .is_none()
    );

    // 5. Workspace balance response decoding clamps negatives and rejects NaN/Infinity/null
    assert_eq!(
        CodexOAuthUsageFetcher::decode_workspace_remaining_balance_response(
            br#"{"balance":"1234"}"#
        )
        .unwrap()
        .balance,
        Some(1234.0)
    );
    assert_eq!(
        CodexOAuthUsageFetcher::decode_workspace_remaining_balance_response(br#"{"balance":-4}"#)
            .unwrap()
            .balance,
        Some(0.0)
    );
    for invalid_json in [
        br#"{}"#.as_slice(),
        br#"{"balance":null}"#,
        br#"{"balance":"NaN"}"#,
        br#"{"balance":"Infinity"}"#,
        br#"{"balance":{"amount":14}}"#,
        br#"{"balance":true}"#,
    ] {
        assert_eq!(
            CodexOAuthUsageFetcher::decode_workspace_remaining_balance_response(invalid_json)
                .unwrap()
                .balance,
            None
        );
    }
}

// ============================================================================
// CodexOAuthRequestTests.swift (httpmock end-to-end & override protection)
// ============================================================================

#[tokio::test]
async fn hostile_chatgpt_base_url_in_config_blocks_oauth_bearer_exfiltration() {
    let server = https::start_server();
    let hostile_mock = server
        .mock_async(|when, then| {
            when.any_request();
            then.status(200).body("{}");
        })
        .await;

    let temp = TempDir::new().unwrap();
    let codex_home = temp.path().join(".codex");
    fs::create_dir_all(&codex_home).unwrap();
    // Point config.toml at an untrusted external host:
    fs::write(
        codex_home.join("config.toml"),
        "chatgpt_base_url = \"https://evil.invalid/backend-api\"\n",
    )
    .unwrap();

    let mut env = BTreeMap::new();
    env.insert(
        "CODEX_HOME".to_owned(),
        codex_home.to_string_lossy().into_owned(),
    );
    let net = https::broker(NetConfig::default());
    let now = "2026-04-18T12:00:00Z".parse::<Timestamp>().unwrap();

    assert_eq!(
        CodexOAuthUsageFetcher::fetch_usage(
            &net,
            "secret-bearer-token",
            Some("acct-1"),
            &env,
            Some(temp.path()),
        )
        .await
        .unwrap_err(),
        CodexOAuthFetchError::InvalidResponse
    );
    assert_eq!(
        CodexOAuthUsageFetcher::fetch_rate_limit_reset_credits(
            &net,
            "secret-bearer-token",
            Some("acct-1"),
            &env,
            Some(temp.path()),
            Duration::from_secs(4),
            now,
        )
        .await
        .unwrap_err(),
        CodexOAuthFetchError::InvalidResponse
    );
    assert_eq!(
        CodexOAuthUsageFetcher::fetch_workspace_remaining_balance(
            &net,
            "secret-bearer-token",
            "acct-1",
            &env,
            Some(temp.path()),
            Duration::from_secs(4),
        )
        .await
        .unwrap_err(),
        CodexOAuthFetchError::InvalidResponse
    );
    assert_eq!(
        CodexOAuthUsageFetcher::fetch_spend_controls_monthly_usage(
            &net,
            "secret-bearer-token",
            "acct-1",
            &env,
            Some(temp.path()),
            Duration::from_secs(4),
        )
        .await
        .unwrap_err(),
        CodexOAuthFetchError::InvalidResponse
    );
    assert_eq!(hostile_mock.calls_async().await, 0);
}

#[tokio::test]
async fn end_to_end_oauth_fetch_over_httpmock() {
    let server = https::start_server();
    let base_url = format!("{}/backend-api", server.base_url());

    let usage_mock = server
        .mock_async(|when, then| {
            when.method(httpmock::Method::GET)
                .path("/backend-api/wham/usage")
                .header("authorization", "Bearer tok-live-test")
                .header("chatgpt-account-id", "ws-override-99");
            then.status(200)
                .header("content-type", "application/json")
                .body(
                    serde_json::to_vec(&json!({
                        "account_id": "ws-override-99",
                        "plan_type": "team",
                        "spend_control": {},
                        "rate_limit": {
                            "primary_window": {
                                "used_percent": 42,
                                "reset_at": 1_800_000_000,
                                "limit_window_seconds": 18_000
                            }
                        },
                        "credits": {
                            "has_credits": true,
                            "unlimited": false
                        }
                    }))
                    .unwrap(),
                );
        })
        .await;

    let reset_mock = server
        .mock_async(|when, then| {
            when.method(httpmock::Method::GET)
                .path("/backend-api/wham/rate-limit-reset-credits")
                .header("openai-beta", "codex-1")
                .header("originator", "Codex Desktop")
                .header("chatgpt-account-id", "ws-override-99");
            then.status(200)
                .header("content-type", "application/json")
                .body(
                    serde_json::to_vec(&json!({
                        "available_count": 1,
                        "credits": [{
                            "id": "grant-e2e-1",
                            "reset_type": "five_hour",
                            "status": "available",
                            "granted_at": "2026-04-10T00:00:00Z",
                            "expires_at": "2026-04-25T00:00:00Z"
                        }]
                    }))
                    .unwrap(),
                );
        })
        .await;

    let balance_mock = server
        .mock_async(|when, then| {
            when.method(httpmock::Method::GET)
                .path("/backend-api/accounts/ws-override-99/remaining_balance")
                .header("chatgpt-account-id", "ws-override-99");
            then.status(200)
                .header("content-type", "application/json")
                .body(r#"{"balance": 88.5}"#);
        })
        .await;

    let spend_mock = server
        .mock_async(|when, then| {
            when.method(httpmock::Method::GET)
                .path("/backend-api/accounts/ws-override-99/spend-controls/current-user/monthly-usage")
                .header("chatgpt-account-id", "ws-override-99");
            then.status(200)
                .header("content-type", "application/json")
                .body(r#"{"current_month_usage": 11.5, "effective_monthly_limit": {"limit": 100, "enforcement_mode": "hard"}}"#);
        })
        .await;

    let temp = TempDir::new().unwrap();
    let codex_home = temp.path().join(".codex");
    fs::create_dir_all(&codex_home).unwrap();
    fs::write(
        codex_home.join("config.toml"),
        format!("chatgpt_base_url = \"{base_url}\"\n"),
    )
    .unwrap();

    let now = "2026-04-18T12:00:00Z".parse::<Timestamp>().unwrap();
    let creds = CodexOAuthCredentials::new(
        "tok-live-test",
        "ref-live-test",
        None,
        Some("acct-initial".to_owned()),
        Some(now),
        None,
        CodexOAuthCredentialSource::CodexHome,
        false,
    );
    let mut env = BTreeMap::new();
    env.insert(
        "CODEX_HOME".to_owned(),
        codex_home.to_string_lossy().into_owned(),
    );

    let net = Arc::new(https::broker(NetConfig::default()));
    let mut cfg = CodexStrategyConfig::new(net, env, temp.path(), temp.path());
    cfg.managed_workspace_account_id = Some("ws-override-99".to_owned());
    cfg.clock = Arc::new(move || now);

    let strategy = CodexOAuthFetchStrategy::new(cfg);
    let mut ctx = make_fetch_context(SourceMode::Oauth);
    ctx.include_credits = true;
    ctx.runtime = FetchRuntime::Cli;

    let outcome = strategy.fetch_with_credentials(&ctx, creds).await.unwrap();
    usage_mock.assert_async().await;
    reset_mock.assert_async().await;
    balance_mock.assert_async().await;
    spend_mock.assert_async().await;

    assert_eq!(outcome.usage.primary.as_ref().unwrap().used_percent, 42.0);
    let credits = outcome.credits.as_ref().unwrap();
    assert_eq!(credits.remaining, 88.5);
    assert!(credits.balance_is_workspace);
    assert_eq!(credits.codex_credit_limit.as_ref().unwrap().limit, 100.0);
    assert_eq!(
        outcome
            .codex_reset_credits
            .as_ref()
            .unwrap()
            .available_inventory(now)
            .count(),
        1
    );
    let _ = CreditEvent::new(now, "test", 1.0);
}
