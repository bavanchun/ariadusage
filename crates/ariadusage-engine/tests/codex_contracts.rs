// Ported from CodexBar Tests/CodexBarTests/CodexPlanFormattingTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/CodexIdentityResolverTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/CodexSpendControlNumberTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/CodexCLILaunchGateTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/CodexCLIBackendConfigurationTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/CodexOAuthTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

use ariadusage_core::pipeline::FetchInteraction;
use ariadusage_engine::providers::codex::*;
use jiff::Timestamp;
use std::time::Duration;

// upstream: CodexPlanFormattingTests.swift:6 maps Codex pro plans to usage multiplier names
#[test]
fn maps_codex_pro_plans_to_usage_multiplier_names() {
    assert_eq!(
        CodexPlanFormatting::display_name("pro").as_deref(),
        Some("Pro 20x")
    );
    assert_eq!(
        CodexPlanFormatting::display_name("Pro").as_deref(),
        Some("Pro 20x")
    );
    assert_eq!(
        CodexPlanFormatting::display_name("Codex Pro").as_deref(),
        Some("Pro 20x")
    );
    assert_eq!(
        CodexPlanFormatting::display_name("prolite").as_deref(),
        Some("Pro 5x")
    );
    assert_eq!(
        CodexPlanFormatting::display_name("pro_lite").as_deref(),
        Some("Pro 5x")
    );
    assert_eq!(
        CodexPlanFormatting::display_name("pro-lite").as_deref(),
        Some("Pro 5x")
    );
    assert_eq!(
        CodexPlanFormatting::display_name("Pro Lite").as_deref(),
        Some("Pro 5x")
    );
    assert_eq!(
        CodexPlanFormatting::display_name("Codex Pro Lite").as_deref(),
        Some("Pro 5x")
    );
}

// upstream: CodexPlanFormattingTests.swift:18 returns nil for empty plan values
#[test]
fn returns_nil_for_empty_plan_values() {
    assert_eq!(
        None::<&str>
            .and_then(CodexPlanFormatting::display_name)
            .as_deref(),
        None
    );
    assert_eq!(CodexPlanFormatting::display_name("").as_deref(), None);
    assert_eq!(CodexPlanFormatting::display_name("   ").as_deref(), None);
}

// upstream: CodexPlanFormattingTests.swift:25 humanizes machine style plan identifiers
#[test]
fn humanizes_machine_style_plan_identifiers() {
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
}

// upstream: CodexPlanFormattingTests.swift:36 preserves unrelated already readable plan text
#[test]
fn preserves_unrelated_already_readable_plan_text() {
    assert_eq!(
        CodexPlanFormatting::display_name("Enterprise").as_deref(),
        Some("Enterprise")
    );
}

// upstream: CodexIdentityResolverTests.swift:5 resolver prefers provider account over email
#[test]
fn resolver_prefers_provider_account_over_email() {
    assert_eq!(
        CodexIdentityResolver::resolve(Some("account-123"), Some("Person@example.com")),
        CodexIdentity::ProviderAccount {
            id: "account-123".into()
        }
    );
}

// upstream: CodexIdentityResolverTests.swift:14 resolver falls back to normalized email when provider account missing
#[test]
fn resolver_falls_back_to_normalized_email_when_provider_account_missing() {
    assert_eq!(
        CodexIdentityResolver::resolve(None, Some(" Person@example.com ")),
        CodexIdentity::EmailOnly {
            normalized_email: "person@example.com".into()
        }
    );
}

// upstream: CodexIdentityResolverTests.swift:23 resolver returns unresolved when account data missing
#[test]
fn resolver_returns_unresolved_when_account_data_missing() {
    assert_eq!(
        CodexIdentityResolver::resolve(None, None),
        CodexIdentity::Unresolved
    );
}

// upstream: CodexIdentityResolverTests.swift:30 provider account does not equal email fallback even when email matches
#[test]
fn provider_account_does_not_equal_email_fallback_even_when_email_matches() {
    let provider = CodexIdentityResolver::resolve(Some("account-123"), Some("person@example.com"));
    let email = CodexIdentityResolver::resolve(None, Some("person@example.com"));
    assert_eq!(
        provider,
        CodexIdentity::ProviderAccount {
            id: "account-123".into()
        }
    );
    assert_eq!(
        email,
        CodexIdentity::EmailOnly {
            normalized_email: "person@example.com".into()
        }
    );
    assert_ne!(provider, email);
}

// upstream: CodexSpendControlNumberTests.swift:6 spend reset parsing preserves truncation and rejects unrepresentable counts
#[test]
fn spend_reset_parsing_preserves_truncation_and_rejects_unrepresentable_counts() {
    for (raw, expected) in [
        ("1782864000", Some(1_782_864_000)),
        ("1782864000.9", Some(1_782_864_000)),
        ("-1.9", Some(-1)),
        (r#"" 1782864000 ""#, Some(1_782_864_000)),
        ("9223372036854775808", None),
        ("1e100", None),
        ("-1e100", None),
        ("null", None),
        ("true", None),
    ] {
        let payload = format!(r#"{{"reset_at":{raw},"limit":" 100 ","used":12.5}}"#);
        let snapshot: SpendControlLimitSnapshot = serde_json::from_str(&payload).unwrap();
        assert_eq!(snapshot.resets_at, expected);
        assert_eq!(snapshot.limit, Some(100.));
        assert_eq!(snapshot.used, Some(12.5));
    }
}

// upstream: CodexCLILaunchGateTests.swift:7 background launch failures suppress repeated background launches until cooldown expires
#[test]
fn background_launch_failures_suppress_repeated_background_launches_until_cooldown_expires() {
    let gate = CodexCLILaunchGate::new();
    let now = Timestamp::from_second(100).unwrap();
    let binary = "/opt/homebrew/bin/codex";
    let message = gate
        .record_launch_failure(
            binary,
            "\"codex\" was not opened because it contains malware.",
            now,
        )
        .unwrap();
    assert!(message.contains("background refresh is paused"));
    assert_eq!(
        gate.background_skip_message(
            binary,
            now.checked_add(Duration::from_secs(60)).unwrap(),
            FetchInteraction::Background
        ),
        Some(message)
    );
    assert!(
        gate.background_skip_message(
            binary,
            now.checked_add(Duration::from_secs(60)).unwrap(),
            FetchInteraction::UserInitiated
        )
        .is_none()
    );
    assert!(
        gate.background_skip_message(
            binary,
            now.checked_add(CodexCLILaunchGate::COOLDOWN + Duration::from_secs(1))
                .unwrap(),
            FetchInteraction::Background
        )
        .is_none()
    );
}

// upstream: CodexCLILaunchGateTests.swift:34 PTY infrastructure failures do not suppress future Codex launches
#[test]
fn pty_infrastructure_failures_do_not_suppress_future_codex_launches() {
    assert!(!CodexCLILaunchGate::should_throttle_launch_failure(
        "openpty failed"
    ));
    assert!(!CodexCLILaunchGate::should_throttle_launch_failure(
        "write to PTY failed"
    ));
    assert!(CodexCLILaunchGate::should_throttle_launch_failure(
        "The operation could not be completed"
    ));
}

// upstream: CodexCLIBackendConfigurationTests.swift:10 amazon bedrock config produces rate limit guidance without login instruction
#[test]
fn amazon_bedrock_config_produces_rate_limit_guidance_without_login_instruction() {
    let config = include_str!("../../../fixtures/codexbar/codex-config-amazon-bedrock.toml");
    let error = CodexCLIBackendRateLimitError::classify(
        "Codex connection failed: codex account authentication required to read rate limits",
        config,
    )
    .unwrap();
    let message = CodexUIErrorMapper::user_facing_message(Some(error.error_description())).unwrap();
    assert_eq!(
        error,
        CodexCLIBackendRateLimitError::ChatGPTRateLimitsUnavailable(CodexCLIBackend::AmazonBedrock)
    );
    assert_eq!(
        message,
        "Codex is configured for Amazon Bedrock; ChatGPT rate limits are unavailable. Disable the Codex usage card or use cost-based tracking."
    );
    assert!(!message.contains("codex login"));
    assert!(!message.contains("device-auth"));
}

// upstream: CodexCLIBackendConfigurationTests.swift:28 normal openai config keeps authentication failure unchanged
#[test]
fn normal_openai_config_keeps_authentication_failure_unchanged() {
    let config = include_str!("../../../fixtures/codexbar/codex-config-openai.toml");
    let error =
        "Codex connection failed: codex account authentication required to read rate limits";
    assert!(CodexCLIBackendRateLimitError::classify(error, config).is_none());
    assert_eq!(
        CodexUIErrorMapper::user_facing_message(Some(error)).as_deref(),
        Some("Codex CLI is not signed in. Run `codex login --device-auth`, then refresh.")
    );
}

// upstream: CodexOAuthTests.swift:761 non 401 invalid grant refresh failure is treated as revoked
#[test]
fn non_401_invalid_grant_refresh_failure_is_treated_as_revoked() {
    assert_eq!(
        CodexTokenRefresher::refresh_failure_error_for_testing(
            400,
            br#"{"error":"invalid_grant"}"#
        ),
        CodexRefreshError::Revoked
    );
}

// upstream: CodexOAuthTests.swift:774 non auth refresh failure remains invalid response
#[test]
fn non_auth_refresh_failure_remains_invalid_response() {
    assert_eq!(
        CodexTokenRefresher::refresh_failure_error_for_testing(
            400,
            br#"{"error":"invalid_request"}"#
        ),
        CodexRefreshError::InvalidResponse("Status 400".into())
    );
}
