// Ported from CodexBar Tests/CodexBarTests/ProviderDiagnosticExportTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

use std::collections::BTreeSet;

use ariadusage_core::model::{NamedRateWindow, RateWindow, UsageSnapshot};
use ariadusage_core::pipeline::*;
use ariadusage_core::providers::{SourceMode, find_by_id_str};
use ariadusage_protocol::metric::SourceKind;
use ariadusage_protocol::usage::{ProviderErrorCategory, ProviderErrorKind};
use ariadusage_protocol::{Confidence, DetailRow, DetailSection, ProviderId};
use jiff::Timestamp;
use serde::{Deserialize, Serialize};

fn to_pretty_json<T: Serialize>(value: &T) -> String {
    serde_json::to_string_pretty(value).expect("encode json")
}

// upstream: Tests/CodexBarTests/ProviderDiagnosticExportTests.swift:6 generic diagnostic export encodes safe provider envelope
#[test]
fn test_generic_diagnostic_export_encodes_safe_provider_envelope() {
    let now = Timestamp::from_second(1_700_000_000).expect("timestamp");
    let reset = Timestamp::from_second(1_700_000_000 + 18000).expect("timestamp");
    let snapshot = UsageSnapshot::new(
        Some(
            RateWindow::new(
                25.0,
                Some(300),
                Some(reset),
                Some("raw local text".to_string()),
                None,
                false,
            )
            .unwrap(),
        ),
        None,
        None,
        None,
        None,
        vec![],
        None,
        None,
        now,
        None,
        Confidence::Unknown,
    )
    .unwrap();
    let export = DiagnosticExport::new(
        now,
        "openai",
        "OpenAI",
        "api",
        "auto",
        DiagnosticAuthSummary::new(true, vec!["api".to_string()]),
        Some(DiagnosticUsageSummary::from_snapshot(&snapshot)),
        vec![DiagnosticFetchAttempt::new(None, "api", None, true, None)],
        None,
        DiagnosticSettingsSummary::new(SourceMode::Auto, None),
    );

    let json = to_pretty_json(&export);

    assert!(json.contains("\"provider\""));
    assert!(json.contains("\"openai\""));
    assert!(json.contains("\"platform\""));
    assert!(json.contains("\"auth\""));
    assert!(json.contains("\"dataConfidence\""));
    assert!(json.contains("\"unknown\""));
    assert!(json.contains("\"hasResetDescription\""));
    assert!(!json.contains("sk-cp-"));
    assert!(!json.contains("sk-api-"));
    assert!(!json.contains("Bearer"));
    assert!(!json.contains("raw local text"));
    assert!(!json.contains("errorMessage"));
    assert!(!json.contains("localizedDescription"));
}

// upstream: Tests/CodexBarTests/ProviderDiagnosticExportTests.swift:51 diagnostic export carries copilot credits detail
#[test]
fn test_diagnostic_export_carries_copilot_credits_detail() {
    let now = Timestamp::from_second(1_700_000_000).expect("timestamp");
    let details = vec![DetailSection {
        title: Some("Credits".to_string()),
        rows: vec![
            DetailRow::new(
                None::<&str>,
                "Credits used",
                "31",
                Some("Resets in 1d"),
                None,
                None,
            )
            .unwrap(),
        ],
        chart: None,
    }];
    let snapshot = UsageSnapshot::new(
        None,
        None,
        None,
        None,
        None,
        details.clone(),
        None,
        None,
        now,
        None,
        Confidence::Unknown,
    )
    .unwrap();
    let summary = DiagnosticUsageSummary::from_snapshot(&snapshot);

    assert_eq!(summary.detail_sections, snapshot.details);
    assert!(
        !summary
            .provider_specific_data
            .contains(&"copilotCredits".to_string())
    );

    let json = to_pretty_json(&summary);
    assert!(json.contains("\"detailSections\""));
    assert!(json.contains("\"Credits used\""));
    assert!(json.contains("31"));
}

// upstream: Tests/CodexBarTests/ProviderDiagnosticExportTests.swift:72 diagnostic export decodes legacy schema without platform metadata
#[test]
fn test_diagnostic_export_decodes_legacy_schema_without_platform_metadata() {
    let export = DiagnosticExport::new(
        Timestamp::from_second(1_700_000_000).expect("timestamp"),
        "openai",
        "OpenAI",
        "api",
        "auto",
        DiagnosticAuthSummary::new(true, vec!["api".to_string()]),
        None,
        vec![],
        None,
        DiagnosticSettingsSummary::new(SourceMode::Auto, None),
    );
    let mut object: serde_json::Map<String, serde_json::Value> =
        serde_json::from_str(&to_pretty_json(&export)).expect("json object");
    object.remove("platform");
    object.remove("appVersion");

    let decoded: DiagnosticExport =
        serde_json::from_value(serde_json::Value::Object(object)).expect("decode");

    assert_eq!(decoded.platform, DiagnosticPlatform::current());
    assert_eq!(decoded.app_version, None);
}

// upstream: Tests/CodexBarTests/ProviderDiagnosticExportTests.swift:167 diagnostic usage summary includes confidence
#[test]
fn test_diagnostic_usage_summary_includes_confidence() {
    let now = Timestamp::from_second(1_700_000_000).expect("timestamp");
    let reset = Timestamp::from_second(1_700_000_000 + 18000).expect("timestamp");
    let snapshot = UsageSnapshot::new(
        Some(RateWindow::new(12.0, Some(300), Some(reset), None, None, false).unwrap()),
        None,
        None,
        None,
        None,
        vec![],
        None,
        None,
        now,
        None,
        Confidence::Exact,
    )
    .unwrap();
    let summary = DiagnosticUsageSummary::from_snapshot(&snapshot);

    assert_eq!(summary.data_confidence, "exact");
}

// upstream: Tests/CodexBarTests/ProviderDiagnosticExportTests.swift:183 diagnostic usage summary defaults legacy payloads to unknown confidence
#[test]
fn test_diagnostic_usage_summary_defaults_legacy_payloads_to_unknown_confidence() {
    let json = r#"{
      "updatedAt": "2023-11-14T22:13:20Z",
      "windows": [],
      "extraWindowCount": 0,
      "providerCostPresent": false,
      "providerSpecificData": []
    }"#;

    let summary: DiagnosticUsageSummary = serde_json::from_str(json).expect("decode");

    assert_eq!(summary.data_confidence, "unknown");
    assert!(to_pretty_json(&summary).contains("\"dataConfidence\": \"unknown\""));
}

// upstream: Tests/CodexBarTests/ProviderDiagnosticExportTests.swift:226 diagnostic export marks named windows with unknown usage
#[test]
fn test_diagnostic_export_marks_named_windows_with_unknown_usage() {
    let now = Timestamp::from_second(1_700_000_000).expect("timestamp");
    let reset = Timestamp::from_second(1_700_000_000 + 3600).expect("timestamp");
    let snapshot = UsageSnapshot::new(
        None,
        None,
        None,
        Some(vec![
            NamedRateWindow::new(
                "nebula-window",
                "Nebula Window",
                RateWindow::new(100.0, None, Some(reset), None, None, false).unwrap(),
            )
            .with_usage_known(false),
        ]),
        None,
        vec![],
        None,
        None,
        now,
        None,
        Confidence::Unknown,
    )
    .unwrap();
    let summary = DiagnosticUsageSummary::from_snapshot(&snapshot);

    let json = to_pretty_json(&summary);
    let object: serde_json::Value = serde_json::from_str(&json).expect("json value");
    let windows = object["windows"].as_array().expect("windows array");

    assert_eq!(
        windows.first().and_then(|w| w["usageKnown"].as_bool()),
        Some(false)
    );
}

// upstream: Tests/CodexBarTests/ProviderDiagnosticExportTests.swift:253 diagnostic rate window defaults legacy payloads to known usage
#[test]
fn test_diagnostic_rate_window_defaults_legacy_payloads_to_known_usage() {
    let json = r#"{
      "label": "Legacy Window",
      "usedPercent": 42,
      "hasResetDescription": false
    }"#;

    let window: DiagnosticRateWindow = serde_json::from_str(json).expect("decode");

    assert!(window.usage_known);
}

// upstream: Tests/CodexBarTests/ProviderDiagnosticExportTests.swift:270 raw error text never appears in encoded JSON
#[test]
fn test_raw_error_text_never_appears_in_encoded_json() {
    let export = DiagnosticExport::new(
        Timestamp::from_second(1_700_000_000).expect("timestamp"),
        "minimax",
        "MiniMax",
        "failed",
        "auto",
        DiagnosticAuthSummary::new(true, vec!["api".to_string()]),
        None,
        vec![DiagnosticFetchAttempt::new(
            None,
            "api",
            None,
            true,
            Some("network".to_string()),
        )],
        Some(DiagnosticError::new(
            "network",
            "Network error - check your connection",
        )),
        DiagnosticSettingsSummary::new(SourceMode::Auto, Some("global".to_string())),
    );

    let json = to_pretty_json(&export);

    assert!(!json.contains("connection refused"));
    assert!(!json.contains("network probe"));
    assert!(!json.contains("not safe to expose"));
    assert!(!json.contains("localizedDescription"));
    assert!(!json.contains("raw"));
    assert!(!json.contains("errorMessage"));
    assert!(json.contains("errorCategory"));
    assert!(json.contains("\"network\""));
}

// upstream: Tests/CodexBarTests/ProviderDiagnosticExportTests.swift:325 diagnostic error maps classified plugin failures
#[test]
fn test_diagnostic_error_maps_classified_plugin_failures() {
    let cases = [
        (ProviderErrorKind::AuthenticationExpired, "auth"),
        (ProviderErrorKind::MissingCredential, "auth"),
        (ProviderErrorKind::PermissionDenied, "auth"),
        (ProviderErrorKind::RateLimited, "api"),
        (ProviderErrorKind::ProviderUnavailable, "api"),
        (ProviderErrorKind::ApiFailure, "api"),
        (ProviderErrorKind::ParseFailure, "parse"),
        (ProviderErrorKind::NetworkFailure, "network"),
    ];

    for (kind, expected_category) in cases {
        let error = ClassifiedError::new(kind, "fixture detail");
        let diagnostic = DiagnosticError::from_classified(&error, true);

        assert_eq!(diagnostic.category, expected_category);
        assert!(!diagnostic.safe_description.contains("fixture detail"));
    }
}

// upstream: Tests/CodexBarTests/ProviderDiagnosticExportTests.swift:344 diagnostic error maps Alibaba invalid endpoint override to configuration
#[test]
fn test_diagnostic_error_maps_alibaba_invalid_endpoint_override_to_configuration() {
    let diag = DiagnosticError::from_description(
        "Invalid endpoint override in ALIBABA_CODING_PLAN_QUOTA_URL",
        true,
    );

    assert_eq!(diag.category, "configuration");
    assert_eq!(
        diag.safe_description,
        "Configuration issue - check provider source and settings"
    );
}

// upstream: Tests/CodexBarTests/ProviderDiagnosticExportTests.swift:353 endpoint override fetch attempt stays in configuration category
#[test]
fn test_endpoint_override_fetch_attempt_stays_in_configuration_category() {
    let desc = "Invalid endpoint override in MINIMAX_HOST";
    let diag_error = DiagnosticError::from_description(desc, true);
    let diag_attempt = DiagnosticFetchAttempt::from_description(
        Some("minimax.web"),
        SourceKind::Web,
        true,
        Some(desc),
    );

    assert_eq!(diag_error.category, "configuration");
    assert_eq!(
        diag_attempt.error_category.as_deref(),
        Some("configuration")
    );
}

// upstream: Tests/CodexBarTests/ProviderDiagnosticExportTests.swift:369 no available strategy maps missing auth to auth category
#[test]
fn test_no_available_strategy_maps_missing_auth_to_auth_category() {
    let error = FetchError::NoAvailableStrategy(ProviderId::new("minimax").unwrap());
    let diag = DiagnosticError::from_error(&error, false);

    assert_eq!(diag.category, "auth");
    assert!(diag.safe_description.contains("Authentication"));
}

// upstream: Tests/CodexBarTests/ProviderDiagnosticExportTests.swift:378 available failed strategy does not imply auth is configured
#[test]
fn test_available_failed_strategy_does_not_imply_auth_is_configured() {
    let outcome = FetchOutcome {
        result: Err(FetchError::NoAvailableStrategy(
            ProviderId::new("antigravity").unwrap(),
        )),
        attempts: vec![FetchAttempt {
            strategy_id: "antigravity.ide-local".to_string(),
            kind: SourceKind::LocalProbe,
            was_available: true,
            failure: Some(AttemptFailure {
                kind: ProviderErrorKind::AuthenticationExpired,
                category: ProviderErrorCategory::Auth,
            }),
        }],
        failure_summary: None,
    };

    let summary = DiagnosticAuthSummary::new(false, vec![]).resolved_with_outcome(&outcome);

    assert!(!summary.configured);
    assert!(summary.modes.is_empty());
}

// upstream: Tests/CodexBarTests/ProviderDiagnosticExportTests.swift:396 fetch attempt error maps to safe category, never raw text
#[test]
fn test_fetch_attempt_error_maps_to_safe_category_never_raw_text() {
    let diag_attempt = DiagnosticFetchAttempt::from_description(
        Some("minimax.api"),
        SourceKind::Api,
        true,
        Some(
            "MiniMax API timeout after 30 seconds - connection refused for host platform.minimax.io",
        ),
    );
    assert_eq!(diag_attempt.kind, "api");
    assert!(diag_attempt.was_available);
    let error_category_one = diag_attempt.error_category.clone();
    assert_eq!(error_category_one.as_deref(), Some("network"));
    let cat1 = error_category_one.unwrap_or_default();
    assert!(!cat1.contains("timeout"));
    assert!(!cat1.contains("connection refused"));
    assert!(!cat1.contains("platform.minimax.io"));

    let diag_auth_attempt = DiagnosticFetchAttempt::from_description(
        Some("minimax.web"),
        SourceKind::Web,
        false,
        Some("invalid auth token cookie HERTZ-SESSION=abc123"),
    );
    assert!(!diag_auth_attempt.was_available);
    let error_category_two = diag_auth_attempt.error_category.clone();
    assert_eq!(error_category_two.as_deref(), Some("auth"));
    let cat2 = error_category_two.unwrap_or_default();
    assert!(!cat2.contains("HERTZ-SESSION"));
}

// upstream: Tests/CodexBarTests/ProviderDiagnosticExportTests.swift:426 fetch attempt carries strategy identity and outcome
#[test]
fn test_fetch_attempt_carries_strategy_identity_and_outcome() {
    let failed_attempt = FetchAttempt {
        strategy_id: "antigravity.app-local".to_string(),
        kind: SourceKind::LocalProbe,
        was_available: true,
        failure: Some(AttemptFailure {
            kind: ProviderErrorKind::ApiFailure,
            category: ProviderErrorCategory::Api,
        }),
    };
    let failed = DiagnosticFetchAttempt::from_attempt(&failed_attempt);
    assert_eq!(failed.strategy_id.as_deref(), Some("antigravity.app-local"));
    assert_eq!(failed.kind, "local");
    assert_eq!(failed.outcome, "failed");

    let skipped_attempt = FetchAttempt {
        strategy_id: "antigravity.cli-https".to_string(),
        kind: SourceKind::Cli,
        was_available: false,
        failure: None,
    };
    let skipped = DiagnosticFetchAttempt::from_attempt(&skipped_attempt);
    assert_eq!(
        skipped.strategy_id.as_deref(),
        Some("antigravity.cli-https")
    );
    assert_eq!(skipped.outcome, "skipped");

    let succeeded_attempt = FetchAttempt {
        strategy_id: "antigravity.oauth".to_string(),
        kind: SourceKind::Oauth,
        was_available: true,
        failure: None,
    };
    let succeeded = DiagnosticFetchAttempt::from_attempt(&succeeded_attempt);
    assert_eq!(succeeded.strategy_id.as_deref(), Some("antigravity.oauth"));
    assert_eq!(succeeded.outcome, "succeeded");
}

// upstream: Tests/CodexBarTests/ProviderDiagnosticExportTests.swift:454 legacy fetch attempt JSON derives outcome without strategy identity
#[test]
fn test_legacy_fetch_attempt_json_derives_outcome_without_strategy_identity() {
    let legacy_json = r#"{"kind":"local","wasAvailable":true,"errorCategory":"api"}"#;
    let legacy: DiagnosticFetchAttempt = serde_json::from_str(legacy_json).expect("decode");

    assert_eq!(legacy.strategy_id, None);
    assert_eq!(legacy.outcome, "failed");

    let legacy_skipped: DiagnosticFetchAttempt =
        serde_json::from_str(r#"{"kind":"cli","wasAvailable":false,"errorCategory":null}"#)
            .expect("decode");
    assert_eq!(legacy_skipped.outcome, "skipped");
}

// upstream: Tests/CodexBarTests/ProviderDiagnosticExportTests.swift:472 serialized fetch outcomes remain compatible with legacy readers
#[test]
fn test_serialized_fetch_outcomes_remain_compatible_with_legacy_readers() {
    #[derive(Serialize, Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct LegacyAttempt {
        kind: String,
        was_available: bool,
        error_category: Option<String>,
    }

    let attempts = vec![
        DiagnosticFetchAttempt::from_description(
            Some("antigravity.app-local"),
            SourceKind::LocalProbe,
            true,
            Some("HTTP failure: sensitive-payload"),
        ),
        DiagnosticFetchAttempt::from_description(
            Some("antigravity.cli-https"),
            SourceKind::Cli,
            false,
            None,
        ),
        DiagnosticFetchAttempt::from_description(
            Some("antigravity.oauth"),
            SourceKind::Oauth,
            true,
            None,
        ),
    ];
    let data = serde_json::to_string(&attempts).expect("encode");
    let object: Vec<serde_json::Value> = serde_json::from_str(&data).expect("decode array");
    let strategy_ids: Vec<&str> = object
        .iter()
        .filter_map(|v| v["strategyID"].as_str())
        .collect();
    assert_eq!(
        strategy_ids,
        vec![
            "antigravity.app-local",
            "antigravity.cli-https",
            "antigravity.oauth"
        ]
    );
    let outcomes: Vec<&str> = object
        .iter()
        .filter_map(|v| v["outcome"].as_str())
        .collect();
    assert_eq!(outcomes, vec!["failed", "skipped", "succeeded"]);
    assert!(!data.contains("sensitive-payload"));

    let legacy: Vec<LegacyAttempt> = serde_json::from_str(&data).expect("decode legacy");
    let kinds: Vec<&str> = legacy.iter().map(|a| a.kind.as_str()).collect();
    assert_eq!(kinds, vec!["local", "cli", "oauth"]);

    let old_data = serde_json::to_string(&legacy).expect("encode legacy");
    let restored: Vec<DiagnosticFetchAttempt> =
        serde_json::from_str(&old_data).expect("restore attempts");
    assert!(restored.iter().all(|a| a.strategy_id.is_none()));
    let restored_outcomes: Vec<&str> = restored.iter().map(|a| a.outcome.as_str()).collect();
    assert_eq!(restored_outcomes, vec!["failed", "skipped", "succeeded"]);
}

// upstream: Tests/CodexBarTests/ProviderDiagnosticExportTests.swift:511 missing api key setup errors map to auth before api
#[test]
fn test_missing_api_key_setup_errors_map_to_auth_before_api() {
    let category = error_category_label(Some(
        "Azure OpenAI API key not configured. Set AZURE_OPENAI_API_KEY.",
    ));
    assert_eq!(category, "auth");
}

// upstream: Tests/CodexBarTests/ProviderDiagnosticExportTests.swift:519 claude subscription notice without quota maps to configuration not auth
#[test]
fn test_claude_subscription_notice_without_quota_maps_to_configuration_not_auth() {
    let desc = "Claude CLI /usage returned a subscription notice without session quota data. Local cost and token history remain available.";
    let category = error_category_label(Some(desc));
    let diagnostic = DiagnosticError::from_classified(
        &ClassifiedError::new(ProviderErrorKind::ParseFailure, desc),
        true,
    );

    assert_eq!(category, "configuration");
    assert_eq!(diagnostic.category, "configuration");
    assert_eq!(
        diagnostic.safe_description,
        "Configuration issue - check provider source and settings"
    );
    assert_eq!(
        error_category_label(Some(&desc.to_ascii_uppercase())),
        "configuration"
    );
}

// upstream: Tests/CodexBarTests/ProviderDiagnosticExportTests.swift:532 fetch attempt coding preserves diagnostic fields
#[test]
fn test_fetch_attempt_coding_preserves_diagnostic_fields() {
    let attempt = DiagnosticFetchAttempt::new(
        Some("claude.cli".to_string()),
        "cli",
        None,
        true,
        Some("configuration".to_string()),
    );
    let data = serde_json::to_string(&attempt).expect("encode");
    let object: serde_json::Map<String, serde_json::Value> =
        serde_json::from_str(&data).expect("decode map");
    let keys: BTreeSet<String> = object.keys().cloned().collect();
    let expected: BTreeSet<String> = [
        "strategyID",
        "kind",
        "outcome",
        "wasAvailable",
        "errorCategory",
    ]
    .into_iter()
    .map(str::to_string)
    .collect();
    assert_eq!(keys, expected);

    let decoded: DiagnosticFetchAttempt = serde_json::from_str(&data).expect("decode attempt");
    assert_eq!(decoded.strategy_id.as_deref(), Some("claude.cli"));
    assert_eq!(decoded.kind, "cli");
    assert_eq!(decoded.outcome, "failed");
    assert!(decoded.was_available);
    assert_eq!(decoded.error_category.as_deref(), Some("configuration"));
}

// upstream: Tests/CodexBarTests/ProviderDiagnosticExportTests.swift:646 builder creates generic safe diagnostic with error on failure
#[test]
fn test_builder_creates_generic_safe_diagnostic_with_error_on_failure() {
    let provider = ProviderId::new("claude").unwrap();
    let descriptor = find_by_id_str("claude").expect("claude descriptor");
    let outcome = FetchOutcome {
        result: Err(FetchError::Classified(ClassifiedError::new(
            ProviderErrorKind::NetworkFailure,
            "timeout",
        ))),
        attempts: vec![FetchAttempt {
            strategy_id: "claude.api".to_string(),
            kind: SourceKind::Api,
            was_available: true,
            failure: Some(AttemptFailure {
                kind: ProviderErrorKind::NetworkFailure,
                category: ProviderErrorCategory::Network,
            }),
        }],
        failure_summary: None,
    };

    let diag = DiagnosticExportBuilder::build(DiagnosticExportBuilderInput {
        provider: &provider,
        descriptor,
        outcome: &outcome,
        source_mode: SourceMode::Auto,
        api_region: None,
        auth: DiagnosticAuthSummary::new(true, vec!["apiToken".to_string()]),
        app_version: Some("9.8.7".to_string()),
        timestamp: Some(Timestamp::from_second(1_700_000_000).unwrap()),
    });

    assert_eq!(diag.provider, "claude");
    assert_eq!(diag.platform, DiagnosticPlatform::current());
    assert_eq!(diag.app_version.as_deref(), Some("9.8.7"));
    assert_eq!(diag.source, "failed");
    assert!(diag.auth.configured);
    assert!(diag.usage.is_none());
    assert!(diag.error.is_some());
    assert_eq!(
        diag.error.as_ref().map(|e| e.category.as_str()),
        Some("network")
    );
    assert_eq!(diag.fetch_attempts.len(), 1);
    assert_eq!(
        diag.fetch_attempts[0].error_category.as_deref(),
        Some("network")
    );
}

// upstream: Tests/CodexBarTests/ProviderDiagnosticExportTests.swift:679 builder creates generic safe diagnostic with MiniMax details on success
#[test]
fn test_builder_creates_generic_safe_diagnostic_on_success() {
    let now = Timestamp::from_second(1_700_000_000).unwrap();
    let reset = Timestamp::from_second(1_700_000_000 + 18000).unwrap();
    let snapshot = UsageSnapshot::new(
        Some(RateWindow::new(25.0, Some(300), Some(reset), None, None, false).unwrap()),
        None,
        None,
        None,
        None,
        vec![DetailSection {
            title: Some("Plan".to_string()),
            rows: vec![
                DetailRow::new(
                    None::<&str>,
                    "Prompts",
                    "250 / 1000",
                    None::<&str>,
                    None,
                    None,
                )
                .unwrap(),
            ],
            chart: None,
        }],
        None,
        None,
        now,
        None,
        Confidence::Unknown,
    )
    .unwrap();

    let result = FetchResult::new(snapshot.clone(), "api", "claude.api", SourceKind::Api);
    let outcome = FetchOutcome {
        result: Ok(result),
        attempts: vec![FetchAttempt {
            strategy_id: "claude.api".to_string(),
            kind: SourceKind::Api,
            was_available: true,
            failure: None,
        }],
        failure_summary: None,
    };
    let provider = ProviderId::new("claude").unwrap();
    let descriptor = find_by_id_str("claude").expect("claude descriptor");

    let diag = DiagnosticExportBuilder::build(DiagnosticExportBuilderInput {
        provider: &provider,
        descriptor,
        outcome: &outcome,
        source_mode: SourceMode::Auto,
        api_region: None,
        auth: DiagnosticAuthSummary::new(true, vec!["apiToken".to_string()]),
        app_version: None,
        timestamp: Some(now),
    });

    assert_eq!(diag.provider, "claude");
    assert_eq!(diag.source, "api");
    assert!(diag.auth.configured);
    assert!(diag.usage.is_some());
    assert!(diag.error.is_none());
    assert!(diag.details.is_none());
    assert_eq!(
        diag.usage.as_ref().map(|u| &u.detail_sections),
        Some(&snapshot.details)
    );
}

#[test]
fn test_diagnostic_export_never_contains_token_cookie_email_or_account_id() {
    let now = Timestamp::from_second(1_700_000_000).unwrap();
    let details = vec![DetailSection {
        title: Some("Account user@example.com".to_string()),
        rows: vec![
            DetailRow::new(
                None::<&str>,
                "Token sk-ant-api03-secret-token",
                concat!("Cookie", ": ", "sessionKey", "=secret-cookie-123"),
                Some("acct_1234567890abcdef Bearer secret-jwt-value"),
                None,
                None,
            )
            .unwrap(),
        ],
        chart: None,
    }];
    let snapshot = UsageSnapshot::new(
        None,
        None,
        None,
        None,
        None,
        details,
        None,
        None,
        now,
        Some(ariadusage_core::model::ProviderIdentitySnapshot::new(
            Some(ProviderId::new("claude").unwrap()),
            Some("owner@example.com".to_string()),
            Some("org-secret-999".to_string()),
            Some("oauth".to_string()),
            Some("acct_secret_account_id".to_string()),
        )),
        Confidence::Exact,
    )
    .unwrap();
    let export = DiagnosticExport::new(
        now,
        "claude",
        "Claude",
        "oauth",
        "auto",
        DiagnosticAuthSummary::new(true, vec!["oauth".to_string()]),
        Some(DiagnosticUsageSummary::from_snapshot(&snapshot)),
        vec![],
        None,
        DiagnosticSettingsSummary::new(SourceMode::Auto, None),
    );

    let json = redact_diagnostic_text(&to_pretty_json(&export));
    assert!(!json.contains("user@example.com"));
    assert!(!json.contains("owner@example.com"));
    assert!(!json.contains("sk-ant-api03-secret-token"));
    assert!(!json.contains("secret-cookie-123"));
    assert!(!json.contains("acct_1234567890abcdef"));
    assert!(!json.contains("acct_secret_account_id"));
    assert!(!json.contains("org-secret-999"));
    assert!(!json.contains("secret-jwt-value"));
    assert!(json.contains("[redacted]"));
}
