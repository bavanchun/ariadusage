// Ported from CodexBar Tests/CodexBarTests/ProviderDiagnosticExportTests.swift at 6a26b2e9b; MIT, see LICENSES/CodexBar-MIT.txt

use ariadusage_core::pipeline::*;
use ariadusage_protocol::ProviderId;
use ariadusage_protocol::metric::SourceKind;
use ariadusage_protocol::usage::{ProviderErrorCategory, ProviderErrorKind};

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

#[test]
fn test_no_available_strategy_maps_missing_auth_to_auth_category() {
    let error = FetchError::NoAvailableStrategy(ProviderId::new("minimax").unwrap());
    let diag = DiagnosticError::from_error(&error, false);

    assert_eq!(diag.category, "auth");
    assert!(diag.safe_description.contains("Authentication"));
}

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
    assert_eq!(failed.strategy_id, "antigravity.app-local");
    assert_eq!(failed.kind, "local");
    assert_eq!(failed.outcome, "failed");

    let skipped_attempt = FetchAttempt {
        strategy_id: "antigravity.cli-https".to_string(),
        kind: SourceKind::Cli,
        was_available: false,
        failure: None,
    };
    let skipped = DiagnosticFetchAttempt::from_attempt(&skipped_attempt);
    assert_eq!(skipped.strategy_id, "antigravity.cli-https");
    assert_eq!(skipped.outcome, "skipped");

    let succeeded_attempt = FetchAttempt {
        strategy_id: "antigravity.oauth".to_string(),
        kind: SourceKind::Oauth,
        was_available: true,
        failure: None,
    };
    let succeeded = DiagnosticFetchAttempt::from_attempt(&succeeded_attempt);
    assert_eq!(succeeded.strategy_id, "antigravity.oauth");
    assert_eq!(succeeded.outcome, "succeeded");
}
