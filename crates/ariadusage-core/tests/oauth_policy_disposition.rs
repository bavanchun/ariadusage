// Ported from CodexBar Tests/CodexBarTests/ClaudeOAuthRefreshDispositionTests.swift at 6a26b2e9b; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/CodexOAuthCredentialReadTests.swift at 6a26b2e9b; MIT, see LICENSES/CodexBar-MIT.txt

use ariadusage_core::oauth::{
    CredentialReadOutcome, RefreshDisposition, extract_oauth_error_code, may_fall_through,
    refresh_disposition,
};

// CodexBar: Tests/CodexBarTests/ClaudeOAuthRefreshDispositionTests.swift:7
#[test]
fn test_invalid_grant_is_terminal() {
    let data = br#"{"error":"invalid_grant"}"#;
    assert_eq!(refresh_disposition(400, data), RefreshDisposition::Terminal);
    assert_eq!(refresh_disposition(401, data), RefreshDisposition::Terminal);
}

// CodexBar: Tests/CodexBarTests/ClaudeOAuthRefreshDispositionTests.swift:14
#[test]
fn test_other_error_is_transient() {
    let data = br#"{"error":"invalid_request"}"#;
    assert_eq!(
        refresh_disposition(400, data),
        RefreshDisposition::Transient
    );
    assert_eq!(
        refresh_disposition(401, data),
        RefreshDisposition::Transient
    );
}

// CodexBar: Tests/CodexBarTests/ClaudeOAuthRefreshDispositionTests.swift:21
#[test]
fn test_undecodable_body_is_transient() {
    let data = b"not-json";
    assert_eq!(
        refresh_disposition(401, data),
        RefreshDisposition::Transient
    );
    assert_eq!(extract_oauth_error_code(data), None);
}

// CodexBar: Tests/CodexBarTests/ClaudeOAuthRefreshDispositionTests.swift:29
#[test]
fn test_non_auth_status_is_not_handled() {
    let data = br#"{"error":"invalid_grant"}"#;
    assert_eq!(
        refresh_disposition(500, data),
        RefreshDisposition::Unhandled
    );
    assert_eq!(
        refresh_disposition(503, data),
        RefreshDisposition::Unhandled
    );
    assert_eq!(
        refresh_disposition(200, data),
        RefreshDisposition::Unhandled
    );
}

// CodexBar: Tests/CodexBarTests/CodexOAuthCredentialReadTests.swift:725
#[test]
fn test_fall_through_only_for_not_found() {
    assert!(may_fall_through(CredentialReadOutcome::NotFound));

    assert!(!may_fall_through(CredentialReadOutcome::Success));
    assert!(!may_fall_through(CredentialReadOutcome::Unreadable));
    assert!(!may_fall_through(CredentialReadOutcome::Untrusted));
    assert!(!may_fall_through(CredentialReadOutcome::TooLarge));
    assert!(!may_fall_through(CredentialReadOutcome::DecodeFailed));
    assert!(!may_fall_through(CredentialReadOutcome::MissingTokens));
}
