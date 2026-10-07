// Ported from CodexBar Tests/CodexBarTests/ClaudeOAuthRefreshChainOwnershipTests.swift at 6a26b2e9b; MIT, see LICENSES/CodexBar-MIT.txt

use ariadusage_core::oauth::{
    OwnershipEvidence, TokenOwner, can_release_to_self_owned, resolve_chain_owner,
};

// CodexBar: Tests/CodexBarTests/ClaudeOAuthRefreshChainOwnershipTests.swift:255
#[test]
fn test_credentials_file_keeps_mirror_cli_owned() {
    let has_cli_file = true;
    for evidence in [
        OwnershipEvidence::SignedIn,
        OwnershipEvidence::SignedOut,
        OwnershipEvidence::Indeterminate,
    ] {
        let resolved = resolve_chain_owner(TokenOwner::SelfOwned, has_cli_file, evidence);
        assert_eq!(resolved, TokenOwner::Cli);
        assert!(!can_release_to_self_owned(has_cli_file, evidence));
    }
}

// CodexBar: Tests/CodexBarTests/ClaudeOAuthRefreshChainOwnershipTests.swift:287
#[test]
fn test_malformed_config_keeps_mirror_cli_owned() {
    let has_cli_file = false;
    let evidence = OwnershipEvidence::Indeterminate;
    let resolved = resolve_chain_owner(TokenOwner::SelfOwned, has_cli_file, evidence);
    assert_eq!(resolved, TokenOwner::Cli);
    assert!(!can_release_to_self_owned(has_cli_file, evidence));
}

// CodexBar: Tests/CodexBarTests/ClaudeOAuthRefreshChainOwnershipTests.swift:327
#[test]
fn test_cleanly_signed_out_config_releases_mirror_to_self_owned() {
    let has_cli_file = false;
    let evidence = OwnershipEvidence::SignedOut;
    let resolved = resolve_chain_owner(TokenOwner::SelfOwned, has_cli_file, evidence);
    assert_eq!(resolved, TokenOwner::SelfOwned);
    assert!(can_release_to_self_owned(has_cli_file, evidence));
}

#[test]
fn test_signed_in_evidence_without_file_keeps_cli_owned() {
    let has_cli_file = false;
    let evidence = OwnershipEvidence::SignedIn;
    let resolved = resolve_chain_owner(TokenOwner::SelfOwned, has_cli_file, evidence);
    assert_eq!(resolved, TokenOwner::Cli);
    assert!(!can_release_to_self_owned(has_cli_file, evidence));
}

#[test]
fn test_non_self_owned_is_unmodified() {
    for non_self in [
        TokenOwner::CliNative,
        TokenOwner::Environment,
        TokenOwner::ReadOnlyExternal,
    ] {
        for has_file in [false, true] {
            for evidence in [
                OwnershipEvidence::SignedIn,
                OwnershipEvidence::SignedOut,
                OwnershipEvidence::Indeterminate,
            ] {
                assert_eq!(resolve_chain_owner(non_self, has_file, evidence), non_self);
            }
        }
    }
}
