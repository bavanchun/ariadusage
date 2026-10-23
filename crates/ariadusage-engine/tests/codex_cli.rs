// Ported from CodexBar Tests/CodexBarTests/CodexCLIWindowNormalizationTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/CodexCLIBackendConfigurationTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/CodexCLILaunchGateTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/CodexUsageFetcherFallbackTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use ariadusage_core::pipeline::{
    FetchContext, FetchError, FetchInteraction, FetchPhase, FetchRuntime, FetchStrategy,
    ProviderPlan,
};
use ariadusage_core::providers::SourceMode;
use ariadusage_engine::brokers::net::{NetBroker, NetConfig};
use ariadusage_engine::brokers::process::AbsolutePath;
use ariadusage_engine::providers::codex::{
    AccountInfo, CODEX_CLI_STRATEGY_ID, CodexAuthBackedAccount, CodexCLIBackend,
    CodexCLIBackendRateLimitError, CodexCLIFetchError, CodexCLILaunchGate, CodexCLIUsageFetcher,
    CodexCLIUsageStrategy, CodexIdentity, CodexNativeCredentialOwnerIdentity,
    CodexOAuthCredentialSource, CodexOAuthCredentialsStore, CodexPlanFormatting, CodexProviderPlan,
    CodexStatusProbeError, CodexStatusSnapshot, CodexStrategyConfig, CodexUIErrorMapper,
    RPCAccountDetails, RPCAccountResponse, RPCRateLimitsResponse, RPCWireError, UsageError,
    UsageLimitsAvailability,
};
use ariadusage_protocol::{Confidence, ProviderId};
use jiff::Timestamp;
use serde_json::json;
use tempfile::TempDir;
use tokio_util::sync::CancellationToken;
use zeroize::Zeroizing;

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

fn fixture_path(rel: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures")
        .join(rel)
}

fn make_fetch_context(source_mode: SourceMode) -> FetchContext {
    FetchContext {
        provider: ProviderId::new("codex").unwrap(),
        runtime: FetchRuntime::Cli,
        interaction: FetchInteraction::UserInitiated,
        phase: FetchPhase::Regular,
        request_id: "test-cli-req".to_owned(),
        source_mode,
        env: BTreeMap::new(),
        include_credits: false,
        selected_token_account: None,
        cancel: CancellationToken::new(),
    }
}

// ============================================================================
// CodexCLIWindowNormalizationTests.swift
// ============================================================================

#[test]
fn maps_rpc_limits_session_and_weekly_and_normalizes_swapped_windows() {
    let now = "2026-04-18T12:00:00Z".parse::<Timestamp>().unwrap();

    // Standard primary 300m, secondary 10080m
    let normal_limits: RPCRateLimitsResponse = serde_json::from_value(json!({
        "rateLimits": {
            "limitId": "codex",
            "primary": {
                "usedPercent": 22.0,
                "windowDurationMins": 300,
                "resetsAt": 1_800_000_000
            },
            "secondary": {
                "usedPercent": 58.0,
                "windowDurationMins": 10_080,
                "resetsAt": 1_800_500_000
            },
            "credits": {
                "hasCredits": true,
                "unlimited": false,
                "balance": "33.75"
            },
            "planType": "prolite"
        }
    }))
    .unwrap();

    let account: RPCAccountResponse = serde_json::from_value(json!({
        "account": {
            "type": "chatgpt",
            "email": "rpc-user@example.test",
            "planType": "pro"
        },
        "requiresOpenaiAuth": true
    }))
    .unwrap();

    let cli_snap =
        CodexCLIUsageFetcher::map_rpc_responses(&normal_limits, Some(&account), now).unwrap();
    let snap = cli_snap.usage.as_ref().unwrap();
    let primary = snap.primary.as_ref().unwrap();
    let secondary = snap.secondary.as_ref().unwrap();
    assert_eq!(primary.used_percent, 22.0);
    assert_eq!(primary.window_minutes, Some(300));
    assert_eq!(secondary.used_percent, 58.0);
    assert_eq!(secondary.window_minutes, Some(10_080));
    let id = snap.identity.as_ref().unwrap();
    assert_eq!(id.account_email.as_deref(), Some("rpc-user@example.test"));
    // rateLimits.planType ("prolite") overrides account.plan ("pro")
    assert_eq!(id.login_method.as_deref(), Some("prolite"));
    assert_eq!(
        CodexPlanFormatting::display_name(id.login_method.as_deref().unwrap()).as_deref(),
        Some("Pro 5x")
    );
    let credits = cli_snap.credits.unwrap();
    assert_eq!(credits.remaining, 33.75);
    assert!(credits.balance_read_succeeded);

    // Swapped windows: primary is 10080m, secondary is 300m -> normalizes so primary=300m, secondary=10080m
    let swapped_snap = CodexCLIUsageFetcher::map_codex_rpc_limits_for_testing(
        Some((80.0, 10_080, Some(1_800_500_000))),
        Some((15.0, 300, Some(1_800_000_000))),
        Some("plus"),
        now,
    )
    .unwrap();
    assert_eq!(swapped_snap.primary.as_ref().unwrap().used_percent, 15.0);
    assert_eq!(
        swapped_snap.primary.as_ref().unwrap().window_minutes,
        Some(300)
    );
    assert_eq!(swapped_snap.secondary.as_ref().unwrap().used_percent, 80.0);
    assert_eq!(
        swapped_snap.secondary.as_ref().unwrap().window_minutes,
        Some(10_080)
    );
}

#[test]
fn maps_rpc_weekly_only_window_and_empty_windows_with_plan_availability() {
    let now = "2026-04-18T12:00:00Z".parse::<Timestamp>().unwrap();
    let codex_id = ProviderId::new("codex").unwrap();

    // Weekly-only primary window (10080m) moves to secondary, leaving primary None
    let weekly_snap = CodexCLIUsageFetcher::map_codex_rpc_limits_for_testing(
        Some((45.0, 10_080, Some(1_800_500_000))),
        None,
        Some("plus"),
        now,
    )
    .unwrap();
    assert!(weekly_snap.primary.is_none());
    assert_eq!(weekly_snap.secondary.as_ref().unwrap().used_percent, 45.0);
    assert_eq!(
        UsageLimitsAvailability::resolve(codex_id.clone(), Some(&weekly_snap), None, None),
        UsageLimitsAvailability::Available
    );

    // Empty windows with plan identity still maps identity and resolves to Unavailable limits
    let empty_snap =
        CodexCLIUsageFetcher::map_codex_rpc_limits_for_testing(None, None, Some("plus"), now)
            .unwrap();
    assert!(empty_snap.primary.is_none());
    assert!(empty_snap.secondary.is_none());
    assert_eq!(
        empty_snap
            .identity
            .as_ref()
            .unwrap()
            .login_method
            .as_deref(),
        Some("plus")
    );
    assert_eq!(
        UsageLimitsAvailability::resolve(codex_id.clone(), Some(&empty_snap), None, None),
        UsageLimitsAvailability::Unavailable
    );

    // NoRateLimitsFound error with account identity resolves to Unavailable
    let acct_with_id = AccountInfo::new(Some("user@example.com".to_owned()), None);
    let no_limits_desc = UsageError::NoRateLimitsFound.to_string();
    assert_eq!(
        UsageLimitsAvailability::resolve(
            codex_id.clone(),
            None,
            Some(&acct_with_id),
            Some(&no_limits_desc)
        ),
        UsageLimitsAvailability::Unavailable
    );

    // NoRateLimitsFound error without account identity resolves to Available
    let acct_without_id = AccountInfo::new(None, None);
    assert_eq!(
        UsageLimitsAvailability::resolve(
            codex_id.clone(),
            None,
            Some(&acct_without_id),
            Some(&no_limits_desc)
        ),
        UsageLimitsAvailability::Available
    );

    // Windowed snapshot overrides stale NoRateLimitsFound error
    assert_eq!(
        UsageLimitsAvailability::resolve(
            codex_id,
            Some(&weekly_snap),
            Some(&acct_with_id),
            Some(&no_limits_desc)
        ),
        UsageLimitsAvailability::Available
    );

    // Empty windows without plan or credits returns NoRateLimitsFound
    let err =
        CodexCLIUsageFetcher::map_codex_rpc_limits_for_testing(None, None, None, now).unwrap_err();
    assert_eq!(err, UsageError::NoRateLimitsFound);
}

#[test]
fn maps_rpc_rate_limits_by_limit_id_and_spend_control_limit() {
    let now = "2026-04-18T12:00:00Z".parse::<Timestamp>().unwrap();

    // rateLimits uses only the top-level rateLimits object for windows (no fallback to rateLimitsByLimitId["codex"]),
    // while rateLimitsByLimitId["codex"] still supplies the spend-control individualLimit for credits.
    let limits_response: RPCRateLimitsResponse = serde_json::from_value(json!({
        "rateLimits": {
            "limitId": "other"
        },
        "rateLimitsByLimitId": {
            "codex": {
                "limitId": "codex",
                "primary": {
                    "usedPercent": 12.0,
                    "windowDurationMins": 300,
                    "resetsAt": 1_800_000_000
                },
                "individualLimit": {
                    "limit": 300,
                    "used": 75,
                    "resetAt": 1_800_900_000
                },
                "planType": "team"
            }
        }
    }))
    .unwrap();

    let cli_snap = CodexCLIUsageFetcher::map_rpc_responses(&limits_response, None, now).unwrap();
    assert!(cli_snap.usage.is_none());
    let credits = cli_snap.credits.unwrap();
    assert!(!credits.balance_read_succeeded);
    let limit = credits.codex_credit_limit.as_ref().unwrap();
    assert_eq!(limit.limit, 300.0);
    assert_eq!(limit.used, 75.0);
    assert_eq!(limit.remaining_percent, 75.0);
}

#[test]
fn recovers_rpc_usage_and_credits_from_prolite_decode_error_body() {
    let now = "2026-04-18T12:00:00Z".parse::<Timestamp>().unwrap();

    let embedded_body = json!({
        "email": "prolite-user@example.test",
        "plan_type": "prolite",
        "rate_limit": {
            "primary_window": {
                "used_percent": 31,
                "reset_at": 1_800_000_000,
                "limit_window_seconds": 18_000
            },
            "secondary_window": {
                "used_percent": 64,
                "reset_at": 1_800_500_000,
                "limit_window_seconds": 604_800
            }
        },
        "credits": {
            "has_credits": true,
            "unlimited": false,
            "balance": "19.5"
        }
    });
    let rpc_err_msg = format!(
        "failed to decode response: unknown variant `prolite`, body={}",
        serde_json::to_string(&embedded_body).unwrap()
    );

    let recovered_usage =
        CodexCLIUsageFetcher::recover_codex_rpc_usage_from_error_for_testing(&rpc_err_msg, now)
            .expect("should recover usage from prolite decode error");
    assert_eq!(recovered_usage.primary.as_ref().unwrap().used_percent, 31.0);
    assert_eq!(
        recovered_usage.secondary.as_ref().unwrap().used_percent,
        64.0
    );
    assert_eq!(recovered_usage.data_confidence, Confidence::Unknown);
    assert_eq!(
        recovered_usage
            .identity
            .as_ref()
            .unwrap()
            .login_method
            .as_deref(),
        Some("prolite")
    );
    assert_eq!(
        recovered_usage
            .identity
            .as_ref()
            .unwrap()
            .account_email
            .as_deref(),
        Some("prolite-user@example.test")
    );

    let recovered_credits =
        CodexCLIUsageFetcher::recover_codex_rpc_credits_from_error_for_testing(&rpc_err_msg, now)
            .expect("should recover credits from prolite decode error");
    assert_eq!(recovered_credits.remaining, 19.5);
    assert!(recovered_credits.balance_read_succeeded);

    // Malformed secondary window is dropped while valid primary window survives:
    let malformed_sec_msg = format!(
        "failed to decode response: body={}",
        serde_json::to_string(&json!({
            "plan_type": "prolite",
            "rate_limit": {
                "primary_window": {
                    "used_percent": 20,
                    "reset_at": 1_800_000_000,
                    "limit_window_seconds": 18_000
                },
                "secondary_window": {
                    "used_percent": "invalid"
                }
            }
        }))
        .unwrap()
    );
    let partial_usage = CodexCLIUsageFetcher::recover_codex_rpc_usage_from_error_for_testing(
        &malformed_sec_msg,
        now,
    )
    .expect("valid primary should survive malformed secondary");
    assert_eq!(partial_usage.primary.as_ref().unwrap().used_percent, 20.0);
    assert!(partial_usage.secondary.is_none());

    // Malformed primary window with weekly-only secondary window returns None because session is missing:
    let malformed_prim_msg = format!(
        "failed to decode response: body={}",
        serde_json::to_string(&json!({
            "plan_type": "prolite",
            "rate_limit": {
                "primary_window": {
                    "used_percent": "invalid"
                },
                "secondary_window": {
                    "used_percent": 50,
                    "reset_at": 1_800_500_000,
                    "limit_window_seconds": 604_800
                }
            }
        }))
        .unwrap()
    );
    assert!(
        CodexCLIUsageFetcher::recover_codex_rpc_usage_from_error_for_testing(
            &malformed_prim_msg,
            now
        )
        .is_none()
    );
}

#[test]
fn maps_codex_status_snapshot_and_rejects_empty_snapshot() {
    let now = "2026-04-18T12:00:00Z".parse::<Timestamp>().unwrap();

    let status = CodexStatusSnapshot {
        credits: Some(15.0),
        five_hour_percent_left: Some(60),
        weekly_percent_left: Some(25),
        five_hour_reset_description: None,
        weekly_reset_description: None,
        five_hour_resets_at: Some(Timestamp::from_second(1_800_000_000).unwrap()),
        weekly_resets_at: Some(Timestamp::from_second(1_800_500_000).unwrap()),
        codex_credit_limit: None,
        raw_text: "status output".to_owned(),
    };
    let mapped = CodexCLIUsageFetcher::map_codex_status_for_testing(&status, now).unwrap();
    assert_eq!(mapped.primary.as_ref().unwrap().used_percent, 40.0);
    assert_eq!(mapped.secondary.as_ref().unwrap().used_percent, 75.0);
    assert_eq!(mapped.data_confidence, Confidence::Unknown);

    // Weekly-only status maps into secondary and leaves primary None
    let weekly_only = CodexStatusSnapshot {
        credits: None,
        five_hour_percent_left: None,
        weekly_percent_left: Some(95),
        five_hour_reset_description: None,
        weekly_reset_description: Some("Resets Feb 6".to_owned()),
        five_hour_resets_at: None,
        weekly_resets_at: None,
        codex_credit_limit: None,
        raw_text: "Weekly limit: 95% left".to_owned(),
    };
    let mapped_weekly =
        CodexCLIUsageFetcher::map_codex_status_for_testing(&weekly_only, now).unwrap();
    assert!(mapped_weekly.primary.is_none());
    assert_eq!(mapped_weekly.secondary.as_ref().unwrap().used_percent, 5.0);
    assert_eq!(
        mapped_weekly.secondary.as_ref().unwrap().window_minutes,
        Some(10_080)
    );
    assert_eq!(
        mapped_weekly
            .secondary
            .as_ref()
            .unwrap()
            .reset_description
            .as_deref(),
        Some("Resets Feb 6")
    );

    // Five-hour-only status maps into primary and leaves secondary None
    let five_hour_only = CodexStatusSnapshot {
        credits: None,
        five_hour_percent_left: Some(69),
        weekly_percent_left: None,
        five_hour_reset_description: Some("Resets 02:09".to_owned()),
        weekly_reset_description: None,
        five_hour_resets_at: None,
        weekly_resets_at: None,
        codex_credit_limit: None,
        raw_text: "5h limit: 69% left".to_owned(),
    };
    let mapped_five_hour =
        CodexCLIUsageFetcher::map_codex_status_for_testing(&five_hour_only, now).unwrap();
    assert_eq!(
        mapped_five_hour.primary.as_ref().unwrap().used_percent,
        31.0
    );
    assert_eq!(
        mapped_five_hour.primary.as_ref().unwrap().window_minutes,
        Some(300)
    );
    assert!(mapped_five_hour.secondary.is_none());

    // Empty status snapshot returns NoRateLimitsFound
    let empty_status = CodexStatusSnapshot {
        credits: None,
        five_hour_percent_left: None,
        weekly_percent_left: None,
        five_hour_reset_description: None,
        weekly_reset_description: None,
        five_hour_resets_at: None,
        weekly_resets_at: None,
        codex_credit_limit: None,
        raw_text: String::new(),
    };
    let err = CodexCLIUsageFetcher::map_codex_status_for_testing(&empty_status, now).unwrap_err();
    assert_eq!(err, UsageError::NoRateLimitsFound);
}

// ============================================================================
// CodexCLIBackendConfigurationTests.swift
// ============================================================================

#[test]
fn classifies_custom_cli_backend_rate_limit_errors_from_fixtures() {
    let bedrock_cfg = fs::read_to_string(fixture_path("codexbar/codex-config-amazon-bedrock.toml"))
        .expect("read amazon bedrock fixture");
    let openai_cfg = fs::read_to_string(fixture_path("codexbar/codex-config-openai.toml"))
        .expect("read openai fixture");

    let rpc_err =
        "account/rateLimits/read failed: codex account authentication required to read rate limits";

    let classified = CodexCLIBackendRateLimitError::classify(rpc_err, &bedrock_cfg)
        .expect("bedrock config should classify auth-required rate limits error");
    assert_eq!(
        classified,
        CodexCLIBackendRateLimitError::ChatGPTRateLimitsUnavailable(CodexCLIBackend::AmazonBedrock)
    );
    assert_eq!(
        classified.error_description(),
        "Codex is configured for Amazon Bedrock; ChatGPT rate limits are unavailable. Disable the Codex usage card or use cost-based tracking."
    );
    // UI error mapper preserves custom backend explanation verbatim
    assert_eq!(
        CodexUIErrorMapper::user_facing_message(Some(classified.error_description())).as_deref(),
        Some(classified.error_description())
    );

    // OpenAI default provider does not trigger custom backend error classification
    assert!(CodexCLIBackendRateLimitError::classify(rpc_err, &openai_cfg).is_none());

    // Normal OpenAI authentication error maps to user-facing sign-in message
    let normal_auth_error =
        "Codex connection failed: codex account authentication required to read rate limits";
    assert_eq!(
        CodexUIErrorMapper::user_facing_message(Some(normal_auth_error)).as_deref(),
        Some("Codex CLI is not signed in. Run `codex login --device-auth`, then refresh.")
    );

    // Non-auth errors do not trigger custom backend classification
    assert!(
        CodexCLIBackendRateLimitError::classify("connection timed out", &bedrock_cfg).is_none()
    );
}

// ============================================================================
// CodexCLILaunchGateTests.swift
// ============================================================================

#[test]
fn launch_gate_throttles_launch_failures_in_background_for_30_mins_except_pty_and_shutdown() {
    let gate = CodexCLILaunchGate::new();
    let binary = "/usr/local/bin/codex";
    let t0 = "2026-04-18T12:00:00Z".parse::<Timestamp>().unwrap();

    // openpty, write to PTY, and app shutdown are NOT throttled
    assert!(!CodexCLILaunchGate::should_throttle_launch_failure(
        "Failed to openpty: Device not configured"
    ));
    assert!(!CodexCLILaunchGate::should_throttle_launch_failure(
        "Error: write to PTY failed"
    ));
    assert!(!CodexCLILaunchGate::should_throttle_launch_failure(
        "aborted due to app shutdown"
    ));
    assert!(CodexCLILaunchGate::should_throttle_launch_failure(
        "The application \"codex\" cannot be opened"
    ));

    // Before recording any failure, background launch is not skipped
    assert!(
        gate.background_skip_message(binary, t0, FetchInteraction::Background)
            .is_none()
    );

    // Record failure at t0
    let throttled = gate
        .record_launch_failure(binary, "spawn failed: blocked", t0)
        .expect("should record throttled failure");
    assert!(throttled.contains("background refresh is paused for 30 minutes"));

    // At t0 + 10m: background launch is skipped, user-initiated launch is allowed
    let t10 = t0.checked_add(Duration::from_secs(600)).unwrap();
    assert!(
        gate.background_skip_message(binary, t10, FetchInteraction::Background)
            .is_some()
    );
    assert!(
        gate.background_skip_message(binary, t10, FetchInteraction::UserInitiated)
            .is_none()
    );

    // At t0 + 30m: cooldown expired, background launch is allowed again
    let t30 = t0.checked_add(CodexCLILaunchGate::COOLDOWN).unwrap();
    assert!(
        gate.background_skip_message(binary, t30, FetchInteraction::Background)
            .is_none()
    );

    // Reset clears failure state immediately
    gate.record_launch_failure(binary, "spawn failed: blocked", t0);
    assert!(
        gate.background_skip_message(binary, t10, FetchInteraction::Background)
            .is_some()
    );
    gate.reset_for_testing();
    assert!(
        gate.background_skip_message(binary, t10, FetchInteraction::Background)
            .is_none()
    );
}

// ============================================================================
// CodexUsageFetcherFallbackTests.swift & Live App-Server JSON-RPC over stdio
// ============================================================================

#[test]
fn auth_backed_account_identity_and_redacted_debug_and_rpc_error_sanitization() {
    let account = AccountInfo::new(
        Some("AuthUser@Example.test".to_owned()),
        Some("pro".to_owned()),
    );
    assert!(account.has_identity());
    assert!(!format!("{account:?}").contains("AuthUser@Example.test"));

    let auth_backed = CodexAuthBackedAccount::new(
        CodexIdentity::ProviderAccount {
            id: "acct-cli-1".to_owned(),
        },
        Some("authuser@example.test".to_owned()),
        Some("pro".to_owned()),
    );
    assert_eq!(auth_backed.email.as_deref(), Some("authuser@example.test"));
    assert_eq!(auth_backed.plan.as_deref(), Some("pro"));
    assert!(!format!("{auth_backed:?}").contains("authuser@example.test"));
    assert!(!format!("{auth_backed:?}").contains("acct-cli-1"));

    // Exact case-sensitive account_id match, case-insensitive email match
    let owner1 =
        CodexNativeCredentialOwnerIdentity::new(Some("acct-cli-1"), Some("authuser@example.test"));
    let owner_same_acct =
        CodexNativeCredentialOwnerIdentity::new(Some("acct-cli-1"), Some("other@example.test"));
    let owner_diff_case_acct =
        CodexNativeCredentialOwnerIdentity::new(Some("ACCT-CLI-1"), Some("authuser@example.test"));
    assert!(owner1.matches(&owner_same_acct));
    assert!(!owner1.matches(&owner_diff_case_acct));

    let rpc_acct_details = RPCAccountDetails::ChatGPT {
        email: "secret-rpc@example.test".to_owned(),
        plan_type: "pro".to_owned(),
    };
    let rpc_acct_resp = RPCAccountResponse {
        account: Some(rpc_acct_details.clone()),
        requires_openai_auth: Some(true),
    };
    assert!(!format!("{rpc_acct_details:?}").contains("secret-rpc@example.test"));
    assert!(!format!("{rpc_acct_resp:?}").contains("secret-rpc@example.test"));

    // RPCWireError::RequestFailed redacts body={...} in Display, Debug, and ClassifiedError
    let wire_err = RPCWireError::RequestFailed(Zeroizing::new(
        "failed to decode response: unknown variant `prolite`, body={\"email\":\"secret-body@example.test\",\"plan_type\":\"prolite\"}".to_owned(),
    ));
    assert!(!wire_err.to_string().contains("secret-body@example.test"));
    assert!(wire_err.to_string().contains("body=[redacted]"));
    assert!(!format!("{wire_err:?}").contains("secret-body@example.test"));
    let cli_err = CodexCLIFetchError::Wire(wire_err);
    let FetchError::Classified(classified_wire) = cli_err.into_fetch_error() else {
        panic!("expected ClassifiedError");
    };
    assert!(!classified_wire.message.contains("secret-body@example.test"));

    // Missing CLI binary reports CodexNotInstalled ("Codex CLI missing...") instead of "Codex not running"
    let temp = TempDir::new().unwrap();
    let mut env = BTreeMap::new();
    env.insert(
        "PATH".to_owned(),
        temp.path().to_string_lossy().into_owned(),
    );
    let missing_fetcher = CodexCLIUsageFetcher::new(
        env,
        Some(temp.path().to_path_buf()),
        temp.path().to_path_buf(),
    );
    let rt = tokio::runtime::Runtime::new().unwrap();
    let now = "2026-04-18T12:00:00Z".parse::<Timestamp>().unwrap();
    let missing_err = rt
        .block_on(missing_fetcher.load_latest_usage(now))
        .unwrap_err();
    assert_eq!(
        missing_err,
        CodexCLIFetchError::StatusProbe(CodexStatusProbeError::CodexNotInstalled)
    );
    assert!(
        CodexStatusProbeError::CodexNotInstalled
            .to_string()
            .starts_with("Codex CLI missing")
    );
    assert!(
        !CodexStatusProbeError::CodexNotInstalled
            .to_string()
            .contains("Codex not running")
    );
}

#[tokio::test]
async fn live_app_server_rpc_fetches_usage_credits_and_account_identity() {
    let temp = TempDir::new().unwrap();
    let codex_home = temp.path().join(".codex");
    fs::create_dir_all(&codex_home).unwrap();

    fs::write(
        codex_home.join("test-app-server.json"),
        serde_json::to_vec(&json!({
            "rate_limits_result": {
                "rateLimits": {
                    "limitId": "codex",
                    "primary": {
                        "usedPercent": 28.0,
                        "windowDurationMins": 300,
                        "resetsAt": 1_800_000_000
                    },
                    "secondary": {
                        "usedPercent": 61.0,
                        "windowDurationMins": 10_080,
                        "resetsAt": 1_800_500_000
                    },
                    "credits": {
                        "hasCredits": true,
                        "unlimited": false,
                        "balance": "54.25"
                    },
                    "individualLimit": {
                        "limit": 200,
                        "used": 45,
                        "resetAt": 1_800_900_000
                    },
                    "planType": "team"
                }
            },
            "account_result": {
                "account": {
                    "type": "chatgpt",
                    "email": "Live.RPC@Example.test",
                    "planType": "team"
                },
                "requiresOpenaiAuth": true
            }
        }))
        .unwrap(),
    )
    .unwrap();

    let mut env = BTreeMap::new();
    env.insert(
        "CODEX_HOME".to_owned(),
        codex_home.to_string_lossy().into_owned(),
    );

    let bin = AbsolutePath::new(env!("CARGO_BIN_EXE_ariadusage-test-child")).unwrap();
    let now = "2026-04-18T12:00:00Z".parse::<Timestamp>().unwrap();

    let fetcher = CodexCLIUsageFetcher::new(
        env.clone(),
        Some(temp.path().to_path_buf()),
        temp.path().to_path_buf(),
    )
    .with_executable(bin.clone());

    // 1. Fetch account snapshot
    let acct_snap = fetcher.load_latest_cli_account_snapshot(now).await.unwrap();
    let id = acct_snap.identity.as_ref().unwrap();
    assert_eq!(id.account_email.as_deref(), Some("Live.RPC@Example.test"));
    assert_eq!(id.login_method.as_deref(), Some("team"));

    // 2. Fetch usage + credits via CodexCLIUsageStrategy and CodexProviderPlan
    let net = Arc::new(NetBroker::new(NetConfig::default()).unwrap());
    let mut cfg = CodexStrategyConfig::new(net, env, temp.path(), temp.path());
    cfg.cli_fetcher = fetcher;
    cfg.clock = Arc::new(move || now);

    let cli_strategy = CodexCLIUsageStrategy::new(cfg.clone());
    let outcome = cli_strategy.fetch_outcome(true, now).await.unwrap();
    assert_eq!(outcome.strategy_id, CODEX_CLI_STRATEGY_ID);
    assert_eq!(outcome.source_label, "codex-cli");
    assert_eq!(outcome.usage.primary.as_ref().unwrap().used_percent, 28.0);
    assert_eq!(outcome.usage.secondary.as_ref().unwrap().used_percent, 61.0);
    let usage_id = outcome.usage.identity.as_ref().unwrap();
    assert_eq!(
        usage_id.account_email.as_deref(),
        Some("Live.RPC@Example.test")
    );
    assert_eq!(usage_id.login_method.as_deref(), Some("team"));

    let credits = outcome.credits.as_ref().unwrap();
    assert_eq!(credits.remaining, 54.25);
    assert_eq!(credits.codex_credit_limit.as_ref().unwrap().limit, 200.0);

    let projected = outcome.project_provider_snapshot(now);
    assert_eq!(
        projected
            .windows
            .primary
            .as_ref()
            .unwrap()
            .value
            .as_ref()
            .unwrap()
            .used_percent,
        28.0
    );
    assert_eq!(
        projected
            .windows
            .secondary
            .as_ref()
            .unwrap()
            .value
            .as_ref()
            .unwrap()
            .used_percent,
        61.0
    );

    let plan = CodexProviderPlan::new(cfg);
    let mut ctx = make_fetch_context(SourceMode::Cli);
    ctx.include_credits = true;
    let strategies = plan.resolve_strategies(&ctx).await;
    assert_eq!(strategies.len(), 1);
    let fetch_res = strategies[0].fetch(&ctx).await.unwrap();
    assert_eq!(fetch_res.strategy_id, CODEX_CLI_STRATEGY_ID);
}

#[tokio::test]
async fn live_app_server_rpc_recovers_prolite_error_and_classifies_bedrock_and_times_out() {
    let temp = TempDir::new().unwrap();
    let codex_home = temp.path().join(".codex");
    fs::create_dir_all(&codex_home).unwrap();

    let bin = AbsolutePath::new(env!("CARGO_BIN_EXE_ariadusage-test-child")).unwrap();
    let now = "2026-04-18T12:00:00Z".parse::<Timestamp>().unwrap();

    let mut env = BTreeMap::new();
    env.insert(
        "CODEX_HOME".to_owned(),
        codex_home.to_string_lossy().into_owned(),
    );

    // 1. Live RPC prolite error recovery
    let prolite_body = json!({
        "email": "prolite@example.test",
        "plan_type": "prolite",
        "rate_limit": {
            "primary_window": {
                "used_percent": 17,
                "reset_at": 1_800_000_000,
                "limit_window_seconds": 18_000
            }
        },
        "credits": {
            "has_credits": true,
            "unlimited": false,
            "balance": "9.0"
        }
    });
    fs::write(
        codex_home.join("test-app-server.json"),
        serde_json::to_vec(&json!({
            "rate_limits_error": {
                "code": -32603,
                "message": format!("failed to decode response: unknown variant `prolite`, body={}", serde_json::to_string(&prolite_body).unwrap())
            },
            "account_result": {
                "account": {
                    "type": "chatgpt",
                    "email": "prolite@example.test",
                    "planType": "pro"
                }
            }
        }))
        .unwrap(),
    )
    .unwrap();

    let fetcher = CodexCLIUsageFetcher::new(
        env.clone(),
        Some(temp.path().to_path_buf()),
        temp.path().to_path_buf(),
    )
    .with_executable(bin.clone());
    let snap = fetcher.load_latest_cli_account_snapshot(now).await.unwrap();
    let usage = snap.usage.unwrap();
    assert_eq!(usage.primary.as_ref().unwrap().used_percent, 17.0);
    assert_eq!(
        usage.identity.as_ref().unwrap().login_method.as_deref(),
        Some("prolite")
    );
    assert_eq!(snap.credits.unwrap().remaining, 9.0);

    // 2. Live RPC custom CLI backend classification (Amazon Bedrock)
    let bedrock_cfg = fs::read_to_string(fixture_path("codexbar/codex-config-amazon-bedrock.toml"))
        .expect("read amazon bedrock fixture");
    fs::write(codex_home.join("config.toml"), bedrock_cfg).unwrap();
    fs::write(
        codex_home.join("test-app-server.json"),
        serde_json::to_vec(&json!({
            "rate_limits_error": {
                "code": -32000,
                "message": "codex account authentication required to read rate limits"
            }
        }))
        .unwrap(),
    )
    .unwrap();

    let err = fetcher
        .load_latest_cli_account_snapshot(now)
        .await
        .unwrap_err();
    assert_eq!(
        err,
        CodexCLIFetchError::BackendRateLimit(
            CodexCLIBackendRateLimitError::ChatGPTRateLimitsUnavailable(
                CodexCLIBackend::AmazonBedrock
            )
        )
    );
    let _ = fs::remove_file(codex_home.join("config.toml"));

    // 3. Live RPC timeout termination when child hangs on rateLimits/read
    fs::write(
        codex_home.join("test-app-server.json"),
        serde_json::to_vec(&json!({
            "hang_on_rate_limits": true
        }))
        .unwrap(),
    )
    .unwrap();

    let fast_timeout_fetcher = CodexCLIUsageFetcher::new(
        env.clone(),
        Some(temp.path().to_path_buf()),
        temp.path().to_path_buf(),
    )
    .with_executable(bin.clone())
    .with_timeouts(Duration::from_secs(2), Duration::from_millis(200));
    for _ in 0..2 {
        let timeout_err = fast_timeout_fetcher
            .load_latest_cli_account_snapshot(now)
            .await
            .unwrap_err();
        assert_eq!(
            timeout_err,
            CodexCLIFetchError::Wire(RPCWireError::Timeout {
                method: "account/rateLimits/read".to_owned()
            })
        );
    }

    // 4. Credits-only RPC error body recovery: load_latest_credits succeeds while load_latest_usage fails with NoRateLimitsFound
    let credits_only_body = json!({
        "email": "prolite-credits@example.test",
        "plan_type": "prolite",
        "credits": {
            "has_credits": true,
            "unlimited": false,
            "balance": "14.5"
        }
    });
    fs::write(
        codex_home.join("test-app-server.json"),
        serde_json::to_vec(&json!({
            "rate_limits_error": {
                "code": -32603,
                "message": format!("failed to decode response: unknown variant `prolite`, body={}", serde_json::to_string(&credits_only_body).unwrap())
            }
        }))
        .unwrap(),
    )
    .unwrap();
    let rec_credits = fetcher.load_latest_credits(now).await.unwrap();
    assert_eq!(rec_credits.remaining, 14.5);
    let rec_usage_err = fetcher.load_latest_usage(now).await.unwrap_err();
    assert_eq!(
        rec_usage_err,
        CodexCLIFetchError::Usage(UsageError::NoRateLimitsFound)
    );

    // 5. Credits-only RPC response without usage windows or plan: load_latest_credits succeeds while load_latest_usage fails with NoRateLimitsFound
    fs::write(
        codex_home.join("test-app-server.json"),
        serde_json::to_vec(&json!({
            "rate_limits_result": {
                "rateLimits": {
                    "limitId": "codex",
                    "credits": {
                        "hasCredits": true,
                        "unlimited": false,
                        "balance": "21.0"
                    }
                }
            }
        }))
        .unwrap(),
    )
    .unwrap();
    let rpc_credits = fetcher.load_latest_credits(now).await.unwrap();
    assert_eq!(rpc_credits.remaining, 21.0);
    let rpc_usage_err = fetcher.load_latest_usage(now).await.unwrap_err();
    assert_eq!(
        rpc_usage_err,
        CodexCLIFetchError::Usage(UsageError::NoRateLimitsFound)
    );

    // 6. Plan-only RPC response without windows returns empty usage snapshot with rate_limits_unavailable=true
    fs::write(
        codex_home.join("test-app-server.json"),
        serde_json::to_vec(&json!({
            "rate_limits_result": {
                "rateLimits": {
                    "limitId": "codex",
                    "planType": "pro"
                }
            },
            "account_result": {
                "account": {
                    "type": "chatgpt",
                    "email": "planonly@example.test",
                    "planType": "plus"
                }
            }
        }))
        .unwrap(),
    )
    .unwrap();
    let plan_only_usage = fetcher.load_latest_usage(now).await.unwrap();
    assert!(plan_only_usage.primary.is_none());
    assert!(plan_only_usage.secondary.is_none());
    assert_eq!(
        UsageLimitsAvailability::resolve(
            ProviderId::new("codex").unwrap(),
            Some(&plan_only_usage),
            None,
            None
        ),
        UsageLimitsAvailability::Unavailable
    );
    assert_eq!(
        plan_only_usage
            .identity
            .as_ref()
            .unwrap()
            .login_method
            .as_deref(),
        Some("pro")
    );

    // 7. Blank/whitespace rateLimits.planType falls back to account.planType
    for blank_plan in [Some(""), Some("   "), None] {
        fs::write(
            codex_home.join("test-app-server.json"),
            serde_json::to_vec(&json!({
                "rate_limits_result": {
                    "rateLimits": {
                        "limitId": "codex",
                        "planType": blank_plan
                    }
                },
                "account_result": {
                    "account": {
                        "type": "chatgpt",
                        "email": "fallbackplan@example.test",
                        "planType": "plus"
                    }
                }
            }))
            .unwrap(),
        )
        .unwrap();
        let fallback_usage = fetcher.load_latest_usage(now).await.unwrap();
        assert_eq!(
            fallback_usage
                .identity
                .as_ref()
                .unwrap()
                .login_method
                .as_deref(),
            Some("plus")
        );
    }

    // 8. load_auth_backed_codex_account reads auth.json credentials
    let jwt = format!(
        "{}.{}.sig",
        base64url_no_pad(r#"{"alg":"none"}"#.as_bytes()),
        base64url_no_pad(
            json!({
                "email": "AuthBacked@Example.test",
                "https://api.openai.com/auth": {
                    "chatgpt_plan_type": "team"
                }
            })
            .to_string()
            .as_bytes()
        )
    );
    fs::write(
        codex_home.join("auth.json"),
        serde_json::to_vec(&json!({
            "tokens": {
                "access_token": "tok-cli",
                "refresh_token": "ref-cli",
                "id_token": jwt,
                "account_id": "acct-cli-backed"
            }
        }))
        .unwrap(),
    )
    .unwrap();
    let loaded_auth_backed = fetcher.load_auth_backed_codex_account();
    assert_eq!(
        loaded_auth_backed.identity,
        CodexIdentity::ProviderAccount {
            id: "acct-cli-backed".to_owned()
        }
    );
    assert_eq!(
        loaded_auth_backed.email.as_deref(),
        Some("authbacked@example.test")
    );
    assert_eq!(loaded_auth_backed.plan.as_deref(), Some("team"));
    let parsed_creds = CodexOAuthCredentialsStore::parse(
        &fs::read(codex_home.join("auth.json")).unwrap(),
        CodexOAuthCredentialSource::CodexHome,
    )
    .unwrap();
    assert_eq!(
        CodexAuthBackedAccount::from_credentials(Some(&parsed_creds)),
        loaded_auth_backed
    );

    // 9. Pre-cancelled FetchContext in CodexCLIUsageStrategy::fetch returns FetchError::Cancelled without recording launch failure,
    // and recorded launch failure pauses Background interaction while allowing UserInitiated interaction.
    let gate = Arc::new(CodexCLILaunchGate::new());
    let mut gated_fetcher = CodexCLIUsageFetcher::new(
        env.clone(),
        Some(temp.path().to_path_buf()),
        temp.path().to_path_buf(),
    )
    .with_executable(bin.clone());
    gated_fetcher.cli_launch_gate = gate.clone();
    let net = Arc::new(NetBroker::new(NetConfig::default()).unwrap());
    let mut cfg = CodexStrategyConfig::new(net, env, temp.path(), temp.path());
    cfg.cli_fetcher = gated_fetcher;
    cfg.clock = Arc::new(move || now);
    let cli_strategy = CodexCLIUsageStrategy::new(cfg);

    let cancelled_ctx = make_fetch_context(SourceMode::Cli);
    cancelled_ctx.cancel.cancel();
    let cancel_err = cli_strategy.fetch(&cancelled_ctx).await.unwrap_err();
    assert!(matches!(cancel_err, FetchError::Cancelled));
    assert!(
        gate.background_skip_message(
            bin.as_path().to_str().unwrap(),
            now,
            FetchInteraction::Background
        )
        .is_none()
    );

    // Record a launch failure on the gate: Background fetch is skipped, UserInitiated fetch proceeds
    fs::write(
        codex_home.join("test-app-server.json"),
        serde_json::to_vec(&json!({
            "rate_limits_result": {
                "rateLimits": {
                    "limitId": "codex",
                    "primary": {
                        "usedPercent": 15.0,
                        "windowDurationMins": 300,
                        "resetsAt": 1_800_000_000
                    },
                    "planType": "pro"
                }
            }
        }))
        .unwrap(),
    )
    .unwrap();
    gate.record_launch_failure(
        bin.as_path().to_str().unwrap(),
        "The application \"codex\" cannot be opened",
        now,
    );

    let mut bg_ctx = make_fetch_context(SourceMode::Cli);
    bg_ctx.interaction = FetchInteraction::Background;
    let bg_err = cli_strategy.fetch(&bg_ctx).await.unwrap_err();
    let FetchError::Classified(classified_bg) = bg_err else {
        panic!("expected ClassifiedError");
    };
    assert!(
        classified_bg
            .message
            .contains("background refresh is paused for 30 minutes")
    );

    let mut fg_ctx = make_fetch_context(SourceMode::Cli);
    fg_ctx.interaction = FetchInteraction::UserInitiated;
    let fg_ok = cli_strategy.fetch(&fg_ctx).await.unwrap();
    assert_eq!(fg_ok.strategy_id, CODEX_CLI_STRATEGY_ID);
}
